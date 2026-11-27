//! Shared, bounded single-poll audit watch inspection for the verifier and M&C adapter.
//!
//! The caller supplies the operator-pinned checkpoint file and a monotonic timestamp. This
//! module never promotes a computed log head to a trust input; only a fresh exact-pin verification
//! can clear a prior alarm. A quiet log is not stale unless the checkpoint publisher declared a
//! positive heartbeat interval.

use super::{LogError, MAX_RECORDS};
use oa_audit_core::{AuditError, Checkpoint};
use oa_techops::{
    record_alarm_transition, Alarm, AlarmSeverity, AlarmState, AlarmTime, AlarmTransition,
    ClockDomainId, ResourceKey, ResourceKind,
};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAX_CONFIG_BYTES: usize = 256;
pub const WATCH_ALARM_COUNT: usize = 5;
const ALARM_COUNT: usize = WATCH_ALARM_COUNT;
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
pub struct WatchConfig {
    pub generation: u64,
    pub checkpoint: Checkpoint,
    pub refresh_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchStatus {
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
pub struct WatchSnapshot {
    pub status: WatchStatus,
    pub generation: Option<u64>,
    pub expected_count: Option<u64>,
    /// Present only after exact count-and-tail verification.
    pub verified_count: Option<u64>,
    pub stale_checkpoint: bool,
    pub checkpoint_diverged: bool,
    pub alarms: [Alarm; ALARM_COUNT],
    /// Fixed alarm rows; cleared rows are empty while the transition event below remains explicit.
    pub transitions: [Option<WatchAlarmTransition>; ALARM_COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchAlarmTransition {
    pub instance_id: u64,
    pub reason_code: u16,
    pub severity: AlarmSeverity,
    pub raised: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchSample {
    pub status: WatchStatus,
    /// Present only after exact count-and-tail verification.
    pub verified_checkpoint: Option<Checkpoint>,
    /// The checkpoint read from the operator-pinned config file.
    pub expected_checkpoint: Option<Checkpoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchError {
    InvalidRecordLimit,
    InvalidMonotonicTime,
    AlarmTransitionRefused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Stateful fixed-capacity poll adapter. Keep one instance in the running status process so
/// divergence debounce, heartbeat freshness, and alarm lifetimes span queries.
pub struct WatchPoller {
    max_records: u64,
    accepted_config: Option<WatchConfig>,
    generation_seen_ns: i64,
    mismatch_signature: Option<(u64, WatchStatus, Option<Checkpoint>)>,
    mismatch_count: u8,
    active: [bool; ALARM_COUNT],
    alarms: [Alarm; ALARM_COUNT],
    alarm_generations: [u32; ALARM_COUNT],
}

impl WatchPoller {
    pub fn new(max_records: u64) -> Result<Self, WatchError> {
        if max_records == 0 || max_records > MAX_RECORDS {
            return Err(WatchError::InvalidRecordLimit);
        }
        Ok(Self {
            max_records,
            accepted_config: None,
            generation_seen_ns: 0,
            mismatch_signature: None,
            mismatch_count: 0,
            active: [false; ALARM_COUNT],
            alarms: [EMPTY_ALARM; ALARM_COUNT],
            alarm_generations: [0; ALARM_COUNT],
        })
    }

    pub fn poll(
        &mut self,
        log_path: &Path,
        config_path: &Path,
        monotonic_now_ns: i64,
    ) -> Result<WatchSnapshot, WatchError> {
        if monotonic_now_ns < 0 {
            return Err(WatchError::InvalidMonotonicTime);
        }
        let config = read_config(config_path).ok();
        let observed_sample = match config {
            Some(value) => inspect_log(log_path, value, self.max_records),
            None => WatchSample {
                status: WatchStatus::VerificationUnavailable,
                verified_checkpoint: None,
                expected_checkpoint: None,
            },
        };
        let config_status = match (self.accepted_config, config) {
            (_, None) => None,
            (None, Some(value)) if observed_sample.status == WatchStatus::Valid => {
                self.accepted_config = Some(value);
                self.generation_seen_ns = monotonic_now_ns;
                None
            }
            (None, Some(_)) => None,
            (Some(accepted), Some(value)) if value.generation < accepted.generation => {
                Some(WatchStatus::GenerationRegression)
            }
            (Some(accepted), Some(value)) if value.generation == accepted.generation => {
                if value == accepted {
                    None
                } else {
                    Some(WatchStatus::GenerationChangedWithoutAdvance)
                }
            }
            (Some(_), Some(value)) if observed_sample.status == WatchStatus::Valid => {
                self.accepted_config = Some(value);
                self.generation_seen_ns = monotonic_now_ns;
                None
            }
            (Some(_), Some(_)) => Some(WatchStatus::GenerationAdvancedUnverified),
        };
        let sample = if let Some(status) = config_status {
            WatchSample {
                status,
                verified_checkpoint: None,
                expected_checkpoint: config.map(|value| value.checkpoint),
            }
        } else {
            observed_sample
        };
        let generation = config.map(|value| value.generation);
        let stale = self.accepted_config.is_some_and(|value| {
            value.refresh_ms > 0
                && monotonic_now_ns.saturating_sub(self.generation_seen_ns)
                    > i64::try_from(value.refresh_ms.saturating_mul(1_000_000)).unwrap_or(i64::MAX)
        }) || (config_status.is_some() || config.is_none()) && self.active[2];
        let signature =
            config.map(|value| (value.generation, sample.status, sample.expected_checkpoint));
        if let Some(signature) = signature {
            if matches!(
                sample.status,
                WatchStatus::CheckpointAhead | WatchStatus::CheckpointUnmatched
            ) {
                if self.mismatch_signature == Some(signature) {
                    self.mismatch_count = self.mismatch_count.saturating_add(1);
                } else {
                    self.mismatch_signature = Some(signature);
                    self.mismatch_count = 1;
                }
            } else {
                self.mismatch_signature = None;
                self.mismatch_count = 0;
            }
        } else {
            self.mismatch_signature = None;
            self.mismatch_count = 0;
        }
        let divergence = self.mismatch_count >= 3;
        let fresh = sample.status == WatchStatus::Valid;
        let [old_chain, old_role, old_stale, old_divergence, old_unavailable] = self.active;
        self.active = [
            sample.status == WatchStatus::ChainBroken || (!fresh && old_chain),
            sample.status == WatchStatus::WrongRoleAction || (!fresh && old_role),
            stale || (generation.is_none() && old_stale),
            divergence || (!fresh && old_divergence),
            config.is_none()
                || matches!(
                    sample.status,
                    WatchStatus::LimitReached
                        | WatchStatus::VerificationUnavailable
                        | WatchStatus::GenerationRegression
                        | WatchStatus::GenerationChangedWithoutAdvance
                        | WatchStatus::GenerationAdvancedUnverified
                )
                || (!fresh && old_unavailable),
        ];
        let transitions = self.update_alarms(self.active, monotonic_now_ns)?;
        Ok(WatchSnapshot {
            status: sample.status,
            generation,
            expected_count: sample.expected_checkpoint.map(|value| value.count),
            verified_count: sample.verified_checkpoint.map(|value| value.count),
            stale_checkpoint: stale,
            checkpoint_diverged: divergence,
            alarms: self.alarms,
            transitions,
        })
    }

    fn update_alarms(
        &mut self,
        active: [bool; ALARM_COUNT],
        now_ns: i64,
    ) -> Result<[Option<WatchAlarmTransition>; ALARM_COUNT], WatchError> {
        let resource = ResourceKey {
            resource_id: 1,
            parent_service_id: 1,
            kind: ResourceKind::Service,
        };
        let mut transitions = [None; ALARM_COUNT];
        for (index, spec) in ALARM_SPECS.iter().copied().enumerate() {
            let Some(is_active) = active.get(index).copied() else {
                return Err(WatchError::AlarmTransitionRefused);
            };
            let Some(slot) = self.alarms.get_mut(index) else {
                return Err(WatchError::AlarmTransitionRefused);
            };
            let Some(generation) = self.alarm_generations.get_mut(index) else {
                return Err(WatchError::AlarmTransitionRefused);
            };
            if is_active && slot.state == AlarmState::Empty {
                *generation = generation
                    .checked_add(1)
                    .ok_or(WatchError::AlarmTransitionRefused)?;
            }
            if !is_active && slot.state == AlarmState::Empty {
                continue;
            }
            let instance = (spec.instance << 32) | u64::from(*generation);
            let transition = record_alarm_transition(
                core::slice::from_mut(slot),
                instance,
                resource,
                spec.reason,
                spec.severity,
                is_active,
                AlarmTime {
                    clock_domain: ClockDomainId(1),
                    at_ns: now_ns,
                },
            )
            .map_err(|_| WatchError::AlarmTransitionRefused)?;
            if transition != AlarmTransition::Unchanged {
                let output = WatchAlarmTransition {
                    instance_id: instance,
                    reason_code: spec.reason,
                    severity: spec.severity,
                    raised: is_active,
                };
                let transition_slot = transitions
                    .get_mut(index)
                    .ok_or(WatchError::AlarmTransitionRefused)?;
                *transition_slot = Some(output);
                if !is_active {
                    *slot = EMPTY_ALARM;
                }
            }
        }
        Ok(transitions)
    }
}

pub fn read_config(path: &Path) -> Result<WatchConfig, String> {
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
        .ok_or("checkpoint config exceeds its fixed buffer")?;
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
    file.read_exact(
        second
            .get_mut(..len)
            .ok_or("checkpoint config exceeds its fixed buffer")?,
    )
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
    parse_config(text)
}

pub fn parse_config(text: &str) -> Result<WatchConfig, String> {
    let mut lines = text.lines();
    if lines.next() != Some("OA-AUDIT-WATCH-V1") {
        return Err("checkpoint config header/version is invalid".into());
    }
    let generation = parse_field(lines.next(), "generation")?;
    let count = parse_field(lines.next(), "count")?;
    let tail = lines
        .next()
        .and_then(|line| line.strip_prefix("tail="))
        .ok_or("checkpoint config is missing tail")?;
    let refresh_ms = parse_field(lines.next(), "refresh_ms")?;
    if lines.next().is_some() || generation == 0 || refresh_ms > 86_400_000 {
        return Err("checkpoint config has extra fields or values outside bounds".into());
    }
    Ok(WatchConfig {
        generation,
        checkpoint: Checkpoint {
            count,
            tail: parse_digest(tail).map_err(|_| "invalid checkpoint tail digest")?,
        },
        refresh_ms,
    })
}

fn parse_field(line: Option<&str>, name: &str) -> Result<u64, String> {
    let prefix = format!("{name}=");
    line.and_then(|value| value.strip_prefix(&prefix))
        .ok_or_else(|| format!("checkpoint config is missing {name}"))?
        .parse()
        .map_err(|_| format!("checkpoint config {name} must be unsigned integer"))
}

pub fn inspect_log(log_path: &Path, config: WatchConfig, max_records: u64) -> WatchSample {
    if config.checkpoint.count > max_records {
        return WatchSample {
            status: WatchStatus::LimitReached,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        };
    }
    match super::verify(log_path, config.checkpoint) {
        Ok(verified_checkpoint) => WatchSample {
            status: WatchStatus::Valid,
            verified_checkpoint: Some(verified_checkpoint),
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::IncompleteRecord { .. }) => WatchSample {
            status: WatchStatus::IncompleteRecord,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::Truncated { sequence }) if sequence <= config.checkpoint.count => {
            WatchSample {
                status: WatchStatus::CheckpointAhead,
                verified_checkpoint: None,
                expected_checkpoint: Some(config.checkpoint),
            }
        }
        Err(LogError::Truncated { .. }) => WatchSample {
            status: WatchStatus::IncompleteRecord,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::BrokenRecord {
            error: AuditError::LogLimitReached,
            ..
        })
        | Err(LogError::Audit(AuditError::LogLimitReached)) => WatchSample {
            status: WatchStatus::LimitReached,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::CheckpointMismatch) => WatchSample {
            status: WatchStatus::CheckpointUnmatched,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::BrokenRecord {
            error: AuditError::RoleDenied,
            ..
        }) => WatchSample {
            status: WatchStatus::WrongRoleAction,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(LogError::BrokenRecord { .. }) => WatchSample {
            status: WatchStatus::ChainBroken,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
        Err(_) => WatchSample {
            status: WatchStatus::VerificationUnavailable,
            verified_checkpoint: None,
            expected_checkpoint: Some(config.checkpoint),
        },
    }
}

fn parse_digest(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 || !text.is_ascii() || text.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err("digest must be 64 lowercase hexadecimal bytes".into());
    }
    let mut digest = [0u8; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let high = pair
            .first()
            .and_then(|byte| hex_nibble(*byte))
            .ok_or("invalid digest hex")?;
        let low = pair
            .get(1)
            .and_then(|byte| hex_nibble(*byte))
            .ok_or("invalid digest hex")?;
        let slot = digest
            .get_mut(index)
            .ok_or("digest index outside fixed buffer")?;
        *slot = (high << 4) | low;
    }
    Ok(digest)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // Test setup uses expect to keep synthetic fixture failures local and legible.
    #![allow(clippy::expect_used)]

    use super::*;
    use oa_audit_core::genesis;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "oa-audit-watch-api-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn write_config(path: &Path, generation: u64, checkpoint: Checkpoint) {
        write_config_with_refresh(path, generation, checkpoint, 0);
    }

    fn write_config_with_refresh(
        path: &Path,
        generation: u64,
        checkpoint: Checkpoint,
        refresh_ms: u64,
    ) {
        let content = format!(
            "OA-AUDIT-WATCH-V1\ngeneration={generation}\ncount={}\ntail={}\nrefresh_ms={refresh_ms}\n",
            checkpoint.count,
            oa_sha256::to_hex(&checkpoint.tail)
        );
        fs::write(path, content).expect("write pinned checkpoint config");
    }

    fn append_synthetic_event(log: &Path, prior: Checkpoint) -> Checkpoint {
        use oa_audit_core::{details, target, Action, Event, Role};

        let (target, target_len) = target(b"svc-1").expect("target");
        let (details, details_len) = details(b"watch test").expect("details");
        super::super::append(
            log,
            prior,
            Event {
                principal: 1,
                role: Role::Configure,
                action: Action::Certification,
                target,
                target_len,
                time_ns: 1,
                details,
                details_len,
            },
        )
        .expect("append synthetic event")
    }

    #[test]
    fn lower_generation_cannot_refresh_or_clear_stale_alarm() {
        let log = temp_path("lower-generation-log");
        let config_path = temp_path("lower-generation-config");
        fs::write(&log, b"").expect("empty log");
        let expected = genesis();
        write_config_with_refresh(&config_path, 5, expected, 1);
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        poller.poll(&log, &config_path, 0).expect("baseline");
        let stale = poller
            .poll(&log, &config_path, 2_000_000)
            .expect("heartbeat expires");
        assert!(stale.stale_checkpoint);
        assert_eq!(
            stale.alarms.get(2).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );

        write_config_with_refresh(&config_path, 4, expected, 1);
        let regressed = poller
            .poll(&log, &config_path, 3_000_000)
            .expect("regressed generation is reported");
        assert_eq!(regressed.status, WatchStatus::GenerationRegression);
        assert_eq!(regressed.verified_count, None);
        assert!(regressed.stale_checkpoint);
        assert_eq!(
            regressed.alarms.get(2).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );
        assert_eq!(
            regressed.alarms.get(4).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );
        assert_eq!(regressed.transitions.get(2).copied().flatten(), None);

        fs::remove_file(&config_path).expect("remove config");
        fs::remove_file(&log).expect("remove log");
    }

    #[test]
    fn same_generation_changed_pin_cannot_refresh_or_clear_stale_alarm() {
        let log = temp_path("same-generation-log");
        let config_path = temp_path("same-generation-config");
        fs::write(&log, b"").expect("empty log");
        let first = genesis();
        write_config_with_refresh(&config_path, 5, first, 1);
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        poller.poll(&log, &config_path, 0).expect("baseline");
        let advanced_log = append_synthetic_event(&log, first);
        write_config_with_refresh(&config_path, 5, advanced_log, 1);
        let changed = poller
            .poll(&log, &config_path, 2_000_000)
            .expect("same-generation pin change is unverified");
        assert_eq!(changed.status, WatchStatus::GenerationChangedWithoutAdvance);
        assert_eq!(changed.verified_count, None);
        assert!(changed.stale_checkpoint);
        assert_eq!(
            changed.alarms.get(2).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );
        assert_eq!(
            changed.alarms.get(4).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );

        fs::remove_file(&config_path).expect("remove config");
        fs::remove_file(&log).expect("remove log");
    }

    #[test]
    fn higher_generation_with_exact_verified_pin_refreshes_and_clears_stale_alarm() {
        let log = temp_path("higher-generation-log");
        let config_path = temp_path("higher-generation-config");
        fs::write(&log, b"").expect("empty log");
        let first = genesis();
        write_config_with_refresh(&config_path, 5, first, 1);
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        poller.poll(&log, &config_path, 0).expect("baseline");
        let stale = poller
            .poll(&log, &config_path, 2_000_000)
            .expect("heartbeat expires");
        assert_eq!(
            stale.alarms.get(2).map(|alarm| alarm.state),
            Some(AlarmState::Active)
        );

        let advanced_log = append_synthetic_event(&log, first);
        write_config_with_refresh(&config_path, 6, advanced_log, 1);
        let recovered = poller
            .poll(&log, &config_path, 3_000_000)
            .expect("higher verified generation recovers");
        assert_eq!(recovered.status, WatchStatus::Valid);
        assert_eq!(recovered.verified_count, Some(1));
        assert!(!recovered.stale_checkpoint);
        assert_eq!(
            recovered.alarms.get(2).map(|alarm| alarm.state),
            Some(AlarmState::Empty)
        );
        assert!(recovered
            .transitions
            .get(2)
            .copied()
            .flatten()
            .is_some_and(|transition| !transition.raised));

        fs::remove_file(&config_path).expect("remove config");
        fs::remove_file(&log).expect("remove log");
    }

    #[test]
    fn watch_config_accepts_exact_schema_and_refuses_unknown_fields() {
        let input = format!(
            "OA-AUDIT-WATCH-V1\ngeneration=1\ncount=0\ntail={}\nrefresh_ms=0\n",
            "00".repeat(32)
        );
        assert_eq!(parse_config(&input).expect("valid config").generation, 1);
        assert!(parse_config(&input.replace("refresh_ms=0", "refresh_ms=0\nextra=1")).is_err());
    }

    // REQ: OA-AUDIT-012
    #[test]
    fn audit_watch_poll_api_debounces_divergence_and_latches_then_clears_alarm() {
        let log = temp_path("empty-log");
        let config_path = temp_path("checkpoint");
        fs::write(&log, []).expect("empty log");
        let expected = genesis();
        let mismatched = Checkpoint {
            count: 1,
            tail: expected.tail,
        };
        write_config(&config_path, 1, mismatched);
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        let first = poller.poll(&log, &config_path, 0).expect("first poll");
        let second = poller
            .poll(&log, &config_path, 1_000_000_000)
            .expect("second poll");
        let third = poller
            .poll(&log, &config_path, 2_000_000_000)
            .expect("third poll");
        assert!(!first.checkpoint_diverged);
        assert!(!second.checkpoint_diverged);
        assert!(third.checkpoint_diverged);
        assert!(third
            .transitions
            .get(3)
            .copied()
            .flatten()
            .is_some_and(|event| event.raised));
        assert_eq!(third.verified_count, None);

        write_config(&config_path, 2, expected);
        let recovered = poller
            .poll(&log, &config_path, 3_000_000_000)
            .expect("fresh exact poll");
        assert_eq!(recovered.status, WatchStatus::Valid);
        assert_eq!(recovered.verified_count, Some(0));
        assert!(!recovered.checkpoint_diverged);
        assert!(recovered
            .transitions
            .get(3)
            .copied()
            .flatten()
            .is_some_and(|event| !event.raised));
        fs::remove_file(&config_path).expect("remove config");
        fs::remove_file(&log).expect("remove log");
    }

    #[cfg(unix)]
    // REQ: OA-AUDIT-012
    #[test]
    fn audit_watch_path_replacement_uses_new_pin_without_claiming_verification() {
        use std::os::unix::fs::symlink;

        let log = temp_path("replacement-log");
        let config_path = temp_path("replacement-config");
        let target = temp_path("replacement-target");
        fs::write(&log, []).expect("empty log");
        write_config(
            &target,
            1,
            Checkpoint {
                count: 0,
                tail: [0x11; 32],
            },
        );
        symlink(&target, &config_path).expect("replace config path with symlink");
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        let sample = poller.poll(&log, &config_path, 0).expect("status poll");
        assert_eq!(sample.status, WatchStatus::CheckpointUnmatched);
        assert_eq!(sample.expected_count, Some(0));
        assert_eq!(sample.verified_count, None);
        fs::remove_file(&config_path).expect("remove symlink");
        fs::remove_file(&target).expect("remove target");
        fs::remove_file(&log).expect("remove log");
    }

    #[cfg(unix)]
    // REQ: OA-AUDIT-012
    #[test]
    fn audit_watch_accepts_symlink_to_valid_unsealed_regular_checkpoint() {
        use std::os::unix::fs::symlink;

        let log = temp_path("symlink-valid-log");
        let config_path = temp_path("symlink-valid-config");
        let target = temp_path("symlink-valid-target");
        fs::write(&log, b"").expect("empty log");
        write_config(&target, 1, genesis());
        symlink(&target, &config_path).expect("symlink to checkpoint");
        let mut poller = WatchPoller::new(8).expect("bounded poller");
        let sample = poller.poll(&log, &config_path, 0).expect("status poll");
        assert_eq!(sample.status, WatchStatus::Valid);
        assert_eq!(sample.expected_count, Some(0));
        assert_eq!(sample.verified_count, Some(0));
        assert_eq!(
            sample
                .alarms
                .iter()
                .filter(|alarm| alarm.state == AlarmState::Active)
                .count(),
            0
        );
        fs::remove_file(&config_path).expect("remove symlink");
        fs::remove_file(&target).expect("remove target");
        fs::remove_file(&log).expect("remove log");
    }
}

//! Bounded headless M&C audit-integrity status query.
//!
//! The caller must keep one `AuditStatusRuntime` alive for continuous divergence debounce,
//! heartbeat age and alarm-instance history. Checkpoint/principal provenance is still an
//! operator claim; this status query does not authenticate a publisher or create a trust anchor.

use oa_audit_log::shell::watch::{WatchPoller, WatchSnapshot, WatchStatus};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_STATUS_POLLS: u64 = 86_400;
const MAX_STATUS_RUNTIME: Duration = Duration::from_secs(86_400);
const MAX_STATUS_OUTPUT_BYTES: usize = 1_048_576;
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct AuditStatusRuntime {
    log_path: PathBuf,
    config_path: PathBuf,
    poller: WatchPoller,
    started: Instant,
    polls: u64,
    output_bytes: usize,
}

impl AuditStatusRuntime {
    pub fn new(log_path: PathBuf, config_path: PathBuf, max_records: u64) -> Result<Self, String> {
        if log_path.as_os_str().len() > 4096 || config_path.as_os_str().len() > 4096 {
            return Err("status path exceeds the 4096-byte bound".into());
        }
        let poller = WatchPoller::new(max_records)
            .map_err(|error| format!("watch setup refused: {error:?}"))?;
        Ok(Self {
            log_path,
            config_path,
            poller,
            started: Instant::now(),
            polls: 0,
            output_bytes: 0,
        })
    }

    pub fn poll(&mut self, out: &mut impl Write) -> Result<WatchSnapshot, String> {
        if self.polls >= MAX_STATUS_POLLS || self.started.elapsed() > MAX_STATUS_RUNTIME {
            return Err("status query exceeded its fixed poll or runtime limit".into());
        }
        let now_ns = i64::try_from(self.started.elapsed().as_nanos())
            .map_err(|_| "status monotonic time exceeds signed nanoseconds")?;
        let snapshot = self
            .poller
            .poll(&self.log_path, &self.config_path, now_ns)
            .map_err(|error| format!("audit status poll refused: {error:?}"))?;
        self.polls = self
            .polls
            .checked_add(1)
            .ok_or("status poll counter overflow")?;
        self.write_snapshot(&snapshot, out)?;
        Ok(snapshot)
    }

    fn write_snapshot(
        &mut self,
        snapshot: &WatchSnapshot,
        out: &mut impl Write,
    ) -> Result<(), String> {
        for transition in snapshot.transitions.iter().flatten() {
            let state = if transition.raised {
                "raised"
            } else {
                "cleared"
            };
            let severity = match transition.severity {
                oa_techops::AlarmSeverity::Informational => "informational",
                oa_techops::AlarmSeverity::Warning => "warning",
                oa_techops::AlarmSeverity::Critical => "critical",
            };
            self.write_line(
                out,
                &format!(
                    "alarm\t{}\t{}\t{}\t{}",
                    transition.instance_id, state, transition.reason_code, severity
                ),
            )?;
        }
        for alarm in snapshot
            .alarms
            .iter()
            .filter(|alarm| alarm.state == oa_techops::AlarmState::Active)
        {
            let severity = match alarm.severity {
                oa_techops::AlarmSeverity::Informational => "informational",
                oa_techops::AlarmSeverity::Warning => "warning",
                oa_techops::AlarmSeverity::Critical => "critical",
            };
            self.write_line(
                out,
                &format!(
                    "active_alarm\t{}\t{}\t{}\t{}",
                    alarm.instance_id, alarm.reason_code, severity, alarm.raised_at_ns
                ),
            )?;
        }
        let status = if snapshot.stale_checkpoint {
            "stale_checkpoint_contract"
        } else if snapshot.checkpoint_diverged {
            "checkpoint_diverged"
        } else {
            status_name(snapshot.status)
        };
        self.write_line(out, &format!(
            "status\t{}\t{}\tverified_count={}\texpected_count={}\tgeneration={}\tstale={}\tdiverged={}\tactive_alarms={}",
            self.polls, status,
            optional_count(snapshot.verified_count), optional_count(snapshot.expected_count),
            snapshot.generation.map_or_else(|| "-".to_owned(), |value| value.to_string()),
            bool_text(snapshot.stale_checkpoint), bool_text(snapshot.checkpoint_diverged),
            snapshot.alarms.iter().filter(|alarm| alarm.state == oa_techops::AlarmState::Active).count()
        ))?;
        Ok(())
    }

    fn write_line(&mut self, out: &mut impl Write, line: &str) -> Result<(), String> {
        let next = self
            .output_bytes
            .checked_add(line.len())
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or("status output counter overflow")?;
        if next > MAX_STATUS_OUTPUT_BYTES {
            return Err("status output exceeded the 1 MiB bound".into());
        }
        writeln!(out, "{line}").map_err(|error| format!("status output failed: {error}"))?;
        self.output_bytes = next;
        Ok(())
    }
}

/// Run a finite status query with persistent alarm state for this process.
/// Arguments after `techops-status`: `<log> <checkpoint-config> <max-polls> <max-records>`.
pub fn run_techops_status(args: &[String], out: &mut impl Write) -> Result<(), String> {
    if args.len() != 4 {
        return Err("usage: oa-certification techops-status <log> <checkpoint-config> <max-polls 1..86400> <max-records 1..65536>".into());
    }
    let max_polls = parse_bounded(
        args.get(2).ok_or("missing max-polls")?,
        1,
        MAX_STATUS_POLLS,
        "max-polls",
    )?;
    let max_records = parse_bounded(
        args.get(3).ok_or("missing max-records")?,
        1,
        oa_audit_log::shell::MAX_RECORDS,
        "max-records",
    )?;
    let log_path = Path::new(args.first().ok_or("missing audit log path")?);
    let config_path = Path::new(args.get(1).ok_or("missing checkpoint config path")?);
    let mut runtime = AuditStatusRuntime::new(
        log_path.to_path_buf(),
        config_path.to_path_buf(),
        max_records,
    )?;
    for poll in 0..max_polls {
        runtime.poll(out)?;
        if poll + 1 < max_polls {
            std::thread::sleep(STATUS_POLL_INTERVAL);
        }
    }
    Ok(())
}

fn parse_bounded(text: &str, min: u64, max: u64, name: &str) -> Result<u64, String> {
    if text.len() > 20 {
        return Err(format!("{name} is too long"));
    }
    let value = text
        .parse::<u64>()
        .map_err(|_| format!("{name} must be unsigned decimal"))?;
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be {min}..={max}"));
    }
    Ok(value)
}

fn optional_count(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn bool_text(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn status_name(status: WatchStatus) -> &'static str {
    match status {
        WatchStatus::Valid => "verified_exact_checkpoint",
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

#[cfg(test)]
mod tests {
    // Synthetic test fixtures use expect to keep setup failures local and readable.
    #![allow(clippy::expect_used)]

    use super::*;
    use oa_audit_core::genesis;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "oa-techops-status-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    // REQ: OA-AUDIT-012
    #[test]
    fn techops_status_query_reports_exact_pinned_audit_state_and_alarm_rows() {
        let log = temp_path("log");
        let config = temp_path("config");
        fs::write(&log, b"").expect("empty log");
        let initial = genesis();
        fs::write(
            &config,
            format!(
                "OA-AUDIT-WATCH-V1\ngeneration=1\ncount=0\ntail={}\nrefresh_ms=0\n",
                oa_sha256::to_hex(&initial.tail)
            ),
        )
        .expect("config");
        let args = vec![
            log.display().to_string(),
            config.display().to_string(),
            "1".into(),
            "64".into(),
        ];
        let mut output = Vec::new();
        run_techops_status(&args, &mut output).expect("status query");
        let text = String::from_utf8(output).expect("UTF-8 output");
        assert!(text.contains("verified_exact_checkpoint"));
        assert!(text.contains("verified_count=0"));
        assert!(text.contains("active_alarms=0"));
        assert!(!text.contains(&oa_sha256::to_hex(&initial.tail)));
        fs::remove_file(&config).expect("remove config");
        fs::remove_file(&log).expect("remove log");
    }

    // REQ: OA-AUDIT-012
    #[test]
    fn techops_status_query_refuses_unbounded_poll_and_record_limits() {
        let error = parse_bounded("86401", 1, MAX_STATUS_POLLS, "max-polls").expect_err("poll cap");
        assert!(error.contains("max-polls"));
        assert!(WatchPoller::new(0).is_err());
    }
}

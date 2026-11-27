//! Operated recorded-session console (shell, B1/B3/C6). Recorded source times remain replay
//! cutoffs; command receipt times are the next recorded cutoff, never a live-clock assertion.

use crate::input::{
    EventKind, Identity, NormalizedEvent, OperatorCommand, SourceKind, MAX_EVENT_LINE_BYTES,
};
use crate::runtime::{
    regional_adaptation_sha256, regional_new, regional_new_from_issued_package, regional_pixels,
    regional_step, RegionalFrame, RegionalRuntime,
};
use crate::session::{flush_receipt_group, push_event, session_cursor_new, ReceiptBatch};
use crate::surveillance::{FRAME_HEIGHT, FRAME_WIDTH, MAX_FRAMES};
use oa_airspace::core::model::Model;
use oa_display::interaction::{FlightIdentity, Intent, TargetView};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const MAX_RECORDING_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const MAX_SCRIPT_ACTIONS: usize = 4096;
pub const MAX_ACTION_BYTES: usize = 128;
pub const MAX_OPERATOR_QUEUE: usize = 8;
pub const MAX_OPERATOR_JOURNAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_FRAME_REPORT_BYTES: usize = 512 * 1024;
const MAX_PROCESSOR_REPORT_BYTES: usize = 1024 * 1024;
const NATIVE_IDLE_LIMIT_SECONDS: u64 = 300;
const DEFAULT_IDLE_LIMIT_SECONDS: u64 = 60;
const MAX_NATIVE_BUFFER_BYTES: usize = 64 * 1024 * 1024;
// A recorded-session demonstration has finite input work even under continuous irrelevant
// evdev traffic. Each poll performs at most one record read and one bounded VT service.
const MAX_NATIVE_POLLS: u32 = 100_000;
const MAX_NATIVE_SESSION_SECONDS: u64 = 1800;

/// A7.2: cold shell action parsed before runtime construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Next,
    Select(u32),
    Actor(u32),
    Intent(Intent),
    Quit,
}

/// Registry/log pin supplied by an external operator host for typed runway commands. The
/// principal is a registry claim; the independently pinned file does not authenticate identity.
#[derive(Clone, Debug)]
pub struct OperatorAuditConfiguration {
    pub registry_path: PathBuf,
    pub registry_sha256: [u8; 32],
    pub audit_log: PathBuf,
    pub checkpoint: oa_audit_core::Checkpoint,
    pub principal_id: u64,
}

/// Non-text typed input scheduled after a script action. This is a host API; no keyboard grammar
/// or claim of controller-display input is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedOperatorInput {
    pub after_action: u32,
    pub intent: Intent,
}

/// A7.2: cold source row with its original ordinal and sequence retained for audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedEvent {
    pub event: NormalizedEvent,
    pub original_sequence: u64,
    pub ordinal: u64,
}

/// A7.2: one immutable admitted receipt group and its original row evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedBatch {
    pub receipt_ns: i64,
    pub source_clock_regressions: u64,
    pub events: Vec<RecordedEvent>,
}

/// A7.2: one explicitly selected full identity captured from the current runner view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedTarget {
    pub view: TargetView,
    pub identity: FlightIdentity,
    pub view_sequence: u32,
}

/// A7.2: pending local action whose generation/identity were captured at selection time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingCommand {
    pub request: OperatorCommand,
    pub identity: FlightIdentity,
    pub centre: [u8; 4],
    pub gufi: u128,
    pub ordinal: u64,
}

/// A7.2: session input mode; native paths are explicit and never opened in script mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleMode {
    Script(PathBuf),
    Device {
        drm: String,
        tty: String,
        connector: u32,
        input_device: String,
        idle_limit_seconds: u64,
    },
}

/// A7.2: bounded command-line configuration.
pub struct ConsoleOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub model: crate::cli::ModelSource,
    pub mode: ConsoleMode,
}

/// A7.2: cold explicit device configuration, checked before device access.
pub struct NativeConsoleConfig<'a> {
    pub drm: &'a str,
    pub tty: &'a str,
    pub connector: u32,
    pub input_device: &'a str,
    pub idle_limit_seconds: u64,
}

/// Parse the minimal explicit console CLI. Exactly one model and one operating mode are required.
pub fn parse_args(args: &[String]) -> Result<ConsoleOptions, String> {
    let mut input = None;
    let mut output = None;
    let mut model: Option<crate::cli::ModelSource> = None;
    let mut issued_package = None;
    let mut issued_manifest_pin = None;
    let mut processor_id = None;
    let mut script = None;
    let mut drm = None;
    let mut tty = None;
    let mut connector = None;
    let mut input_device = None;
    let mut idle = DEFAULT_IDLE_LIMIT_SECONDS;
    let mut index = 0usize;
    while index < args.len() {
        let key = args.get(index).ok_or("missing option")?.as_str();
        let value = |i: &mut usize| -> Result<&str, String> {
            *i = i.checked_add(1).ok_or("argument index overflow")?;
            args.get(*i)
                .map(String::as_str)
                .ok_or_else(|| format!("{key} requires a value"))
        };
        match key {
            "--input" if input.is_none() => input = Some(PathBuf::from(value(&mut index)?)),
            "--out" if output.is_none() => output = Some(PathBuf::from(value(&mut index)?)),
            "--synthetic-model" if model.is_none() => {
                model = Some(crate::cli::ModelSource::Synthetic)
            }
            "--snapshot" if model.is_none() => {
                model = Some(crate::cli::ModelSource::Snapshot(PathBuf::from(value(
                    &mut index,
                )?)))
            }
            "--issued-package" if issued_package.is_none() => {
                issued_package = Some(PathBuf::from(value(&mut index)?))
            }
            "--issued-manifest-sha256" if issued_manifest_pin.is_none() => {
                issued_manifest_pin = Some(value(&mut index)?.to_owned())
            }
            "--processor-id" if processor_id.is_none() => {
                processor_id = Some(
                    value(&mut index)?
                        .parse::<u32>()
                        .map_err(|_| "processor ID must be a positive integer")?,
                )
            }
            "--script" if script.is_none() => script = Some(PathBuf::from(value(&mut index)?)),
            "--drm" if drm.is_none() => drm = Some(value(&mut index)?.to_owned()),
            "--tty" if tty.is_none() => tty = Some(value(&mut index)?.to_owned()),
            "--connector" if connector.is_none() => {
                let text = value(&mut index)?;
                let parsed = text
                    .strip_prefix("0x")
                    .map_or_else(|| text.parse::<u32>(), |hex| u32::from_str_radix(hex, 16))
                    .map_err(|_| "invalid connector id")?;
                if parsed == 0 {
                    return Err("connector id must be positive".into());
                }
                connector = Some(parsed);
            }
            "--input-device" if input_device.is_none() => {
                input_device = Some(value(&mut index)?.to_owned())
            }
            "--idle" => {
                let text = value(&mut index)?;
                idle = text
                    .parse::<u64>()
                    .map_err(|_| "idle limit must be seconds")?;
                if idle == 0 || idle > NATIVE_IDLE_LIMIT_SECONDS {
                    return Err("idle limit must be 1..=300 seconds".into());
                }
            }
            _ => return Err(format!("unknown, duplicated or misplaced option: {key}")),
        }
        index += 1;
    }
    let input = input.ok_or("--input is required")?;
    let output = output.ok_or("--out is required")?;
    let issued_fields_present =
        issued_package.is_some() || issued_manifest_pin.is_some() || processor_id.is_some();
    if model.is_some() && issued_fields_present {
        return Err("choose one model source only".into());
    }
    if issued_fields_present {
        let root = issued_package.ok_or("--issued-package is required")?;
        let pin = issued_manifest_pin.ok_or("--issued-manifest-sha256 is required")?;
        let expected_manifest_sha256 = oa_adapt::manifest::parse_pin(&pin)
            .map_err(|_| "invalid issued manifest SHA-256 pin")?;
        let processor_id = processor_id.ok_or("--processor-id is required")?;
        if processor_id == 0 {
            return Err("processor ID must be positive".into());
        }
        model = Some(crate::cli::ModelSource::IssuedPackage {
            root,
            expected_manifest_sha256,
            processor_id,
        });
    }
    let model = model.ok_or(
        "choose --synthetic-model, --snapshot FILE, or --issued-package with an independent pin",
    )?;
    let mode = if let Some(script) = script {
        if drm.is_some() || tty.is_some() || connector.is_some() || input_device.is_some() {
            return Err("--script cannot be combined with device options".into());
        }
        ConsoleMode::Script(script)
    } else {
        ConsoleMode::Device {
            drm: drm.ok_or("device mode requires --drm")?,
            tty: tty.ok_or("device mode requires --tty")?,
            connector: connector.ok_or("device mode requires --connector")?,
            input_device: input_device.ok_or("device mode requires --input-device")?,
            idle_limit_seconds: idle,
        }
    };
    Ok(ConsoleOptions {
        input,
        output,
        model,
        mode,
    })
}

/// Parse the ASCII command grammar. Empty lines are ignored; every nonempty row is one action.
pub fn parse_action(line: &str) -> Result<Action, String> {
    if line.len() > MAX_ACTION_BYTES
        || !line.is_ascii()
        || line.bytes().any(|b| b.is_ascii_control() && b != b'\t')
    {
        return Err("action must be at most 128 printable ASCII bytes".into());
    }
    let fields: Vec<_> = line.split_ascii_whitespace().collect();
    match fields.as_slice() {
        ["NEXT"] => Ok(Action::Next),
        ["QUIT"] => Ok(Action::Quit),
        ["SELECT", address] => {
            let address = address.strip_prefix("0x").unwrap_or(address);
            let parsed =
                u32::from_str_radix(address, 16).map_err(|_| "invalid hexadecimal address")?;
            if parsed == 0 || parsed > 0x01ff_ffff {
                return Err("SELECT requires a positive full address".into());
            }
            Ok(Action::Select(parsed))
        }
        ["ACTOR", unit] => {
            let parsed = unit.parse::<u32>().map_err(|_| "ACTOR must be 1 or 2")?;
            if !(1..=2).contains(&parsed) {
                return Err("ACTOR must be 1 or 2".into());
            }
            Ok(Action::Actor(parsed))
        }
        ["ASSIGNED", feet] => Ok(Action::Intent(Intent::SetAssignedAltitude(
            feet.parse::<i32>()
                .map_err(|_| "ASSIGNED requires integer feet")?,
        ))),
        ["UNASSIGNED"] => Ok(Action::Intent(Intent::ClearAssignedAltitude)),
        ["INTERIM", feet] => Ok(Action::Intent(Intent::SetInterimAltitude(
            feet.parse::<i32>()
                .map_err(|_| "INTERIM requires integer feet")?,
        ))),
        ["CLEAR_INTERIM"] => Ok(Action::Intent(Intent::ClearInterimAltitude)),
        ["HO_OFFER", unit] => Ok(Action::Intent(Intent::OfferHandoff(parse_unit(unit)?))),
        ["HO_ACCEPT", unit] => Ok(Action::Intent(Intent::AcceptHandoff(parse_unit(unit)?))),
        ["HO_TAKE", unit] => Ok(Action::Intent(Intent::TakeHandoff(parse_unit(unit)?))),
        ["HO_CANCEL", unit] => Ok(Action::Intent(Intent::CancelHandoff(parse_unit(unit)?))),
        _ => Err("unknown or malformed console action".into()),
    }
}

fn parse_unit(text: &str) -> Result<u32, String> {
    let unit = text
        .parse::<u32>()
        .map_err(|_| "handoff unit must be 1 or 2")?;
    if !(1..=2).contains(&unit) {
        return Err("handoff unit must be 1 or 2".into());
    }
    Ok(unit)
}

/// Parse and cap a script before opening any device or output path.
pub fn parse_actions(bytes: &[u8]) -> Result<Vec<Action>, String> {
    if bytes.len() > MAX_SCRIPT_BYTES {
        return Err("script exceeds 64 KiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "script is not UTF-8")?;
    let mut actions = Vec::with_capacity(MAX_SCRIPT_ACTIONS);
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if actions.len() >= MAX_SCRIPT_ACTIONS {
            return Err("script exceeds 4096 actions".into());
        }
        actions.push(parse_action(line)?);
    }
    if actions.is_empty() {
        return Err("script contains no actions".into());
    }
    if actions
        .iter()
        .enumerate()
        .any(|(ix, action)| matches!(action, Action::Quit) && ix + 1 != actions.len())
    {
        return Err("QUIT must be the final script action".into());
    }
    Ok(actions)
}

/// Parse recorded input and preserve original row order/sequence evidence before any device access.
pub fn parse_recording(bytes: &[u8]) -> Result<Vec<RecordedBatch>, String> {
    if bytes.len() > MAX_RECORDING_BYTES {
        return Err("recording exceeds 16 MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "recording is not UTF-8")?;
    if text.lines().next() != Some("OA-RS\t1") {
        return Err("expected OA-RS header".into());
    }
    let mut cursor = session_cursor_new();
    let mut pending: Vec<RecordedEvent> = Vec::with_capacity(crate::MAX_EVENTS_PER_RECEIPT);
    let mut batches = Vec::with_capacity(MAX_FRAMES);
    let mut ordinal = 0u64;
    for line in text.lines().skip(1).filter(|line| !line.is_empty()) {
        if line.len() > MAX_EVENT_LINE_BYTES {
            return Err("recording event row exceeds input line limit".into());
        }
        ordinal = ordinal.checked_add(1).ok_or("recording ordinal overflow")?;
        if ordinal > crate::MAX_SESSION_EVENTS as u64 {
            return Err("recording exceeds 100,000 normalized events".into());
        }
        let event = crate::parse_event_line(line).map_err(|e| format!("input: {e:?}"))?;
        let recorded = RecordedEvent {
            original_sequence: event.sequence,
            event,
            ordinal,
        };
        let complete = push_event(&mut cursor, event).map_err(|e| format!("input order: {e:?}"))?;
        if let Some(batch) = complete {
            if batches.len() >= MAX_FRAMES {
                return Err("recording exceeds 512 receipt groups".into());
            }
            batches.push(RecordedBatch {
                receipt_ns: batch.receipt_ns,
                source_clock_regressions: batch.source_clock_regressions,
                events: core::mem::take(&mut pending),
            });
        }
        if pending.len() >= crate::MAX_EVENTS_PER_RECEIPT {
            return Err("receipt group exceeds 256 rows".into());
        }
        pending.push(recorded);
    }
    if let Some(batch) =
        flush_receipt_group(&mut cursor).map_err(|e| format!("input order: {e:?}"))?
    {
        if batches.len() >= MAX_FRAMES {
            return Err("recording exceeds 512 receipt groups".into());
        }
        batches.push(RecordedBatch {
            receipt_ns: batch.receipt_ns,
            source_clock_regressions: batch.source_clock_regressions,
            events: pending,
        });
    }
    if batches.is_empty() {
        return Err("recording contains no receipt groups".into());
    }
    Ok(batches)
}

fn source_ix(source: SourceKind) -> usize {
    match source {
        SourceKind::Surveillance => 0,
        SourceKind::Flight => 1,
        SourceKind::Route => 2,
        SourceKind::Weather => 3,
        SourceKind::Adaptation => 4,
    }
}

fn source_text(source: SourceKind) -> &'static str {
    crate::input::source_name(source)
}

/// Rebuild one batch with local commands between existing Flight and Route rows. All source
/// sequences are assigned monotonically per category; every changed original value is journaled.
pub fn prepare_batch(
    recorded: &RecordedBatch,
    commands: &[PendingCommand],
    next_sequence: &mut [u64; 5],
    journal: &mut OperatorJournal,
) -> Result<ReceiptBatch, String> {
    if recorded.events.len().saturating_add(commands.len()) > crate::MAX_EVENTS_PER_RECEIPT {
        return Err("receipt batch plus queued local commands exceeds 256 events".into());
    }
    if commands.len() > MAX_OPERATOR_QUEUE {
        return Err("local command queue exceeds eight".into());
    }
    let mut staged: Vec<(Option<RecordedEvent>, Option<PendingCommand>)> =
        Vec::with_capacity(recorded.events.len() + commands.len());
    let mut inserted = false;
    for &source in &recorded.events {
        if !inserted && source.event.source > SourceKind::Flight {
            staged.extend(
                commands
                    .iter()
                    .copied()
                    .map(|command| (None, Some(command))),
            );
            inserted = true;
        }
        staged.push((Some(source), None));
    }
    if !inserted {
        staged.extend(
            commands
                .iter()
                .copied()
                .map(|command| (None, Some(command))),
        );
    }
    let mut events = Vec::with_capacity(staged.len());
    for (original, command) in staged {
        let (mut event, original_sequence, ordinal) = if let Some(row) = original {
            (row.event, row.original_sequence, row.ordinal)
        } else if let Some(command) = command {
            (
                NormalizedEvent {
                    receipt_ns: recorded.receipt_ns,
                    source_ns: recorded.receipt_ns,
                    sequence: 0,
                    source: SourceKind::Flight,
                    identity: Identity {
                        flight_gufi: command.gufi,
                        aircraft_address: command.identity.aircraft_address,
                    },
                    centre: command.centre,
                    kind: EventKind::Operator(command.request),
                },
                0,
                command.ordinal,
            )
        } else {
            return Err("empty staged event".into());
        };
        let ix = source_ix(event.source);
        let slot = next_sequence.get_mut(ix).ok_or("source category index")?;
        *slot = slot.checked_add(1).ok_or("source sequence overflow")?;
        let assigned = *slot;
        if original.is_some() {
            if assigned != original_sequence {
                operator_journal_push(
                    journal,
                    format!(
                        "RESEQUENCE\t{ordinal}\t{}\t{}\t{assigned}\t{}\n",
                        source_text(event.source),
                        original_sequence,
                        event.receipt_ns
                    ),
                )?;
            }
        } else {
            operator_journal_push(
                journal,
                format!(
                    "LOCAL\t{ordinal}\tFLIGHT\t0\t{assigned}\t{}\n",
                    event.receipt_ns
                ),
            )?;
        }
        event.sequence = assigned;
        events.push(event);
    }
    Ok(ReceiptBatch {
        receipt_ns: recorded.receipt_ns,
        source_clock_regressions: recorded.source_clock_regressions,
        events,
    })
}

/// A7.2: cold operator evidence. The only storage is one capped byte vector.
#[derive(Debug)]
pub struct OperatorJournal {
    bytes: Vec<u8>,
}

pub fn operator_journal_new() -> OperatorJournal {
    OperatorJournal {
        bytes: Vec::with_capacity(4096),
    }
}

pub fn operator_journal_push(journal: &mut OperatorJournal, row: String) -> Result<(), String> {
    let next = journal
        .bytes
        .len()
        .checked_add(row.len())
        .ok_or("operator journal size overflow")?;
    if next > MAX_OPERATOR_JOURNAL_BYTES {
        return Err("operator journal exceeds 16 MiB".into());
    }
    journal.bytes.extend_from_slice(row.as_bytes());
    Ok(())
}

pub fn operator_journal_bytes(journal: &OperatorJournal) -> &[u8] {
    &journal.bytes
}

/// Checks address uniqueness and captures the exact positive flight identity at the current view.
pub fn selected_target(
    views: &[TargetView],
    address: u32,
    view_sequence: u32,
) -> Result<SelectedTarget, String> {
    if views.len() > crate::flight::MAX_DRIVER_FLIGHTS {
        return Err("selection view exceeds the 32-flight work budget".into());
    }
    let mut matching = views.iter().filter(|view| view.address == address);
    let view = *matching
        .next()
        .ok_or("selection address is not in the current controller view")?;
    if matching.next().is_some() {
        return Err("selection address is duplicated in the current view".into());
    }
    if view.address == 0
        || view.identity_claims != 1
        || view.identity_present != 1
        || view.generation == 0
    {
        return Err(
            "selection requires one positively bound full identity and current generation".into(),
        );
    }
    Ok(SelectedTarget {
        view,
        identity: view.identity,
        view_sequence,
    })
}

/// Build an explicit local command from the selected full identity and actor.
pub fn make_command(
    selected: SelectedTarget,
    actor_unit: u32,
    intent: Intent,
    ordinal: u64,
) -> Result<PendingCommand, String> {
    if !(1..=2).contains(&actor_unit) {
        return Err("actor must be local development unit 1 or 2".into());
    }
    let request = OperatorCommand {
        plan_id: selected.identity.plan_id,
        target_generation: selected.view.generation,
        view_sequence: selected.view_sequence,
        actor_unit,
        intent,
    };
    Ok(PendingCommand {
        request,
        identity: selected.identity,
        centre: selected.identity.centre,
        gufi: u128::from_be_bytes(selected.identity.gufi),
        ordinal,
    })
}

#[derive(Clone, Copy)]
struct ClockStamp {
    wall_ms: i64,
    monotonic_ns: u64,
}

fn clock_stamp(start: Instant) -> Result<ClockStamp, String> {
    let wall_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("wall clock before Unix epoch: {e}"))?
        .as_millis()
        .try_into()
        .map_err(|_| "wall clock millisecond overflow")?;
    let monotonic_ns = start
        .elapsed()
        .as_nanos()
        .try_into()
        .map_err(|_| "monotonic clock nanosecond overflow")?;
    Ok(ClockStamp {
        wall_ms,
        monotonic_ns,
    })
}

fn input_snapshot_key_sha256(snapshot: &oa_display_present::InputSnapshot) -> [u8; 32] {
    oa_sha256::sha256(&snapshot.key_bits)
}

fn device_actions_enabled(lifecycle: oa_display_present::KmsLifecycle) -> bool {
    lifecycle == oa_display_present::KmsLifecycle::Active
}

fn is_idle_input_timeout(error: &oa_display_present::InputDeviceError) -> bool {
    *error == oa_display_present::InputDeviceError::Timeout
}

struct SessionRunner {
    runtime: RegionalRuntime,
    local_activation: Option<oa_adapt::local_inventory::LocalActivation>,
    processor_report: String,
    batches: Vec<RecordedBatch>,
    next_batch: usize,
    next_sequence: [u64; 5],
    next_action: u64,
    selected: Option<SelectedTarget>,
    actor: Option<u32>,
    queue: Vec<PendingCommand>,
    current_frame: Option<RegionalFrame>,
    journal: OperatorJournal,
    frame_manifest: Vec<String>,
    output: PathBuf,
    settings_sha256: [u8; 32],
    console_redraws: usize,
    wall_start: Instant,
    input_loss_unresolved: bool,
    runway_audit: Option<RunwayAuditState>,
}

struct RunwayAuditState {
    registry: oa_audit_log::shell::PinnedPrincipalRegistry,
    audit_log: PathBuf,
    checkpoint: oa_audit_core::Checkpoint,
    principal_id: u64,
}

impl SessionRunner {
    fn action(&mut self, action: Action, stamp: ClockStamp) -> Result<bool, String> {
        if self.next_action >= MAX_SCRIPT_ACTIONS as u64 {
            return Err("operator session exceeds 4096 actions".into());
        }
        self.next_action = self
            .next_action
            .checked_add(1)
            .ok_or("operator action ordinal overflow")?;
        let ordinal = self.next_action;
        let cutoff = self
            .current_frame
            .as_ref()
            .map_or(0, |frame| frame.surveillance.time_ns);
        operator_journal_push(
            &mut self.journal,
            format!(
                "ACTION\t{ordinal}\t{}\t{}\t{cutoff}\t{action:?}\n",
                stamp.wall_ms, stamp.monotonic_ns
            ),
        )?;
        if self.current_frame.is_none() && !matches!(action, Action::Next | Action::Quit) {
            self.refusal(
                ordinal,
                "advance one recorded receipt before selecting or issuing actions",
            )?;
            return Ok(false);
        }
        match action {
            Action::Next => self.advance(ordinal)?,
            Action::Select(address) => self.select(address, ordinal)?,
            Action::Actor(unit) => {
                self.actor = Some(unit);
                operator_journal_push(
                    &mut self.journal,
                    format!(
                        "ACTOR\t{ordinal}\t{unit}\t{}\t{}\n",
                        stamp.wall_ms, stamp.monotonic_ns
                    ),
                )?;
                self.update_overlay(&format!("ACTOR {unit}"), ordinal, stamp)?;
            }
            Action::Intent(intent) => self.queue_intent(intent, ordinal, stamp)?,
            Action::Quit => {
                if !self.queue.is_empty() {
                    return Err("QUIT with queued local commands is unresolved".into());
                }
                if self.input_loss_unresolved {
                    return Err("QUIT before evdev resynchronization is incomplete".into());
                }
                operator_journal_push(
                    &mut self.journal,
                    format!(
                        "QUIT\t{ordinal}\t{}\t{}\n",
                        stamp.wall_ms, stamp.monotonic_ns
                    ),
                )?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn advance(&mut self, ordinal: u64) -> Result<(), String> {
        let Some(recorded) = self.batches.get(self.next_batch).cloned() else {
            self.refusal(ordinal, "no recorded receipt group remains")?;
            return Ok(());
        };
        let mut batch_full_refused = 0usize;
        let queued = if recorded.events.len().saturating_add(self.queue.len())
            > crate::MAX_EVENTS_PER_RECEIPT
        {
            let refused_commands = core::mem::take(&mut self.queue);
            batch_full_refused = refused_commands.len();
            for command in &refused_commands {
                self.refusal(
                    command.ordinal,
                    "queued command refused: receipt batch full",
                )?;
            }
            Vec::new()
        } else {
            core::mem::take(&mut self.queue)
        };
        let mut batch = prepare_batch(
            &recorded,
            &queued,
            &mut self.next_sequence,
            &mut self.journal,
        )?;
        let mut runway_applied = 0usize;
        let mut runway_refused = 0usize;
        let mut runway_intent_uncommitted = 0usize;
        let mut runway_audit_uncertain = 0usize;
        for event in &batch.events {
            let crate::input::EventKind::Operator(command) = &event.kind else {
                continue;
            };
            if !matches!(
                command.intent,
                Intent::AssignRunway(_) | Intent::AcknowledgeRunwayAlert
            ) {
                continue;
            }
            let Some(audit) = self.runway_audit.as_mut() else {
                runway_refused = runway_refused
                    .checked_add(1)
                    .ok_or("runway refusal counter overflow")?;
                operator_journal_push(
                    &mut self.journal,
                    format!(
                        "RUNWAY_COMMAND\t{}\tREFUSED\tpinned audit configuration unavailable\n",
                        event.sequence
                    ),
                )?;
                continue;
            };
            let identity = FlightIdentity {
                gufi: event.identity.flight_gufi.to_be_bytes(),
                aircraft_address: event.identity.aircraft_address,
                centre: event.centre,
                plan_id: command.plan_id,
            };
            let time_ns = clock_stamp(self.wall_start)?.monotonic_ns.max(1);
            let result = crate::runtime::regional_dispatch_runway_operator(
                &mut self.runtime,
                crate::runtime::RunwayOperatorDispatchRequest {
                    operator_command: *command,
                    identity,
                    generation: command.target_generation,
                    service_sequence: u32::try_from(event.sequence)
                        .map_err(|_| "runway command sequence exceeds u32")?,
                    audit_log: &audit.audit_log,
                    expected_checkpoint: audit.checkpoint,
                    principal_registry: &audit.registry,
                    principal: audit.principal_id,
                    time_ns,
                },
            );
            match result {
                Ok(receipt) => {
                    audit.checkpoint = match receipt {
                        crate::runtime::RunwayOperatorReceipt::Assignment(receipt) => {
                            receipt.audit_checkpoint
                        }
                        crate::runtime::RunwayOperatorReceipt::AlertAcknowledgement(receipt) => {
                            receipt.audit_checkpoint
                        }
                    };
                    runway_applied = runway_applied
                        .checked_add(1)
                        .ok_or("runway applied counter overflow")?;
                    operator_journal_push(
                        &mut self.journal,
                        format!(
                            "RUNWAY_COMMAND\t{}\tAPPLIED\tcheckpoint={}\n",
                            event.sequence, audit.checkpoint.count
                        ),
                    )?;
                }
                Err(reason) if reason.contains("durable") => {
                    runway_intent_uncommitted = runway_intent_uncommitted
                        .checked_add(1)
                        .ok_or("runway uncommitted counter overflow")?;
                    operator_journal_push(
                        &mut self.journal,
                        format!(
                            "RUNWAY_COMMAND\t{}\tDURABLE_INTENT_NOT_COMMITTED\t{}\n",
                            event.sequence,
                            reason.replace(['\t', '\n'], " ")
                        ),
                    )?;
                }
                Err(reason) if reason.contains("audit append") => {
                    runway_audit_uncertain = runway_audit_uncertain
                        .checked_add(1)
                        .ok_or("runway audit uncertainty counter overflow")?;
                    operator_journal_push(
                        &mut self.journal,
                        format!(
                            "RUNWAY_COMMAND\t{}\tAUDIT_OUTCOME_UNCERTAIN\t{}\n",
                            event.sequence,
                            reason.replace(['\t', '\n'], " ")
                        ),
                    )?;
                }
                Err(reason) => {
                    runway_refused = runway_refused
                        .checked_add(1)
                        .ok_or("runway refusal counter overflow")?;
                    operator_journal_push(
                        &mut self.journal,
                        format!(
                            "RUNWAY_COMMAND\t{}\tREFUSED\t{}\n",
                            event.sequence,
                            reason.replace(['\t', '\n'], " ")
                        ),
                    )?;
                }
            }
        }
        batch.events.retain(|event| {
            !matches!(
                &event.kind,
                crate::input::EventKind::Operator(command)
                    if matches!(command.intent, Intent::AssignRunway(_) | Intent::AcknowledgeRunwayAlert)
            )
        });
        let mut frame = regional_step(&mut self.runtime, &batch)?;
        self.next_batch += 1;
        self.selected = None; // Every command requires selection against the newest view sequence.
        if frame.controller_dispatches.len() > crate::MAX_EVENTS_PER_RECEIPT {
            return Err("command outcome presentation exceeds receipt work budget".into());
        }
        let selection_note = if frame.controller_dispatches.is_empty()
            && batch_full_refused == 0
            && runway_applied == 0
            && runway_refused == 0
            && runway_intent_uncommitted == 0
            && runway_audit_uncertain == 0
        {
            "SELECT then issue command".to_owned()
        } else {
            let accepted = frame
                .controller_dispatches
                .iter()
                .filter(|dispatch| {
                    dispatch.refusal.is_empty()
                        && dispatch.outcome.as_ref().is_some_and(|outcome| {
                            outcome.adapter_refusal.is_none()
                                && matches!(
                                    outcome.operation,
                                    Some(
                                        oa_flight::Outcome::Applied | oa_flight::Outcome::Unchanged
                                    )
                                )
                        })
                })
                .count();
            let refused = (frame.controller_dispatches.len() - accepted)
                .checked_add(batch_full_refused)
                .ok_or("command refusal count overflow")?;
            if runway_applied == 0
                && runway_refused == 0
                && runway_intent_uncommitted == 0
                && runway_audit_uncertain == 0
            {
                format!(
                    "COMMANDS ACCEPTED {accepted} REFUSED {}; SELECT AGAIN",
                    refused
                )
            } else {
                format!(
                    "COMMANDS ACCEPTED {} REFUSED {}; DURABLE-INTENT-NOT-COMMITTED {}; AUDIT-OUTCOME-UNCERTAIN {}; SELECT AGAIN",
                    accepted + runway_applied,
                    refused + runway_refused + batch_full_refused,
                    runway_intent_uncommitted,
                    runway_audit_uncertain
                )
            }
        };
        self.clear_runtime_selection(&format!("FRAME:{}", frame.sequence))?;
        self.reserve_presentation_update()?;
        let digest = crate::runtime::regional_console_overlay(&mut self.runtime, &selection_note)?;
        frame.controller_selection_kind = oa_display::interaction::SelectionKind::None;
        frame.controller_selected_address = None;
        frame.controller_selection = None;
        frame.console_text = selection_note;
        frame.surveillance.frame_sha256 = digest;
        operator_journal_push(
            &mut self.journal,
            format!(
                "OVERLAY\tFRAME\t{}\t{}\n",
                frame.sequence,
                oa_sha256::to_hex(&digest)
            ),
        )?;
        let (report_hash, ppm_hash) = save_frame(&self.output, &frame, &self.runtime)?;
        self.frame_manifest.push(format!(
            "frame\t{}\t{}\tframe-{:04}.txt\t{}\tframe-{:04}.ppm\t{}\n",
            frame.sequence,
            frame.surveillance.time_ns,
            frame.sequence,
            oa_sha256::to_hex(&report_hash),
            frame.sequence,
            oa_sha256::to_hex(&ppm_hash)
        ));
        self.current_frame = Some(frame);
        Ok(())
    }

    fn select(&mut self, address: u32, ordinal: u64) -> Result<(), String> {
        let Some(frame) = &self.current_frame else {
            return self.refusal(ordinal, "SELECT requires a completed current frame");
        };
        let view_sequence = u32::try_from(frame.sequence).map_err(|_| "view sequence overflow")?;
        let selected = match selected_target(&frame.controller_views, address, view_sequence) {
            Ok(selected) => selected,
            Err(reason) => {
                self.selected = None;
                self.clear_runtime_selection("SELECTION_REFUSAL")?;
                self.refusal(ordinal, &reason)?;
                return Ok(());
            }
        };
        self.reserve_presentation_update()?;
        match crate::runtime::regional_select(
            &mut self.runtime,
            address,
            selected.view.generation,
            Some(selected.identity),
        ) {
            Ok(digest) => {
                self.selected = Some(selected);
                operator_journal_push(
                    &mut self.journal,
                    format!(
                        "SELECT\t{ordinal}\t{address:08x}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        selected.view.generation,
                        selected.view_sequence,
                        oa_sha256::to_hex(&selected.identity.gufi),
                        oa_sha256::to_hex(&selected.identity.centre),
                        oa_sha256::to_hex(&selected.identity.plan_id),
                        oa_sha256::to_hex(&digest)
                    ),
                )?;
                self.update_overlay(
                    &format!("SELECT {address:08X}"),
                    ordinal,
                    clock_stamp(self.wall_start)?,
                )?;
            }
            Err(reason) => {
                self.selected = None;
                self.refusal(ordinal, &reason)?;
            }
        }
        Ok(())
    }

    fn queue_intent(
        &mut self,
        intent: Intent,
        ordinal: u64,
        stamp: ClockStamp,
    ) -> Result<(), String> {
        let Some(actor) = self.actor else {
            self.refusal(ordinal, "set ACTOR 1 or ACTOR 2 first")?;
            return Ok(());
        };
        let Some(selected) = self.selected else {
            self.refusal(ordinal, "SELECT one bound current flight before commanding")?;
            return Ok(());
        };
        if self.queue.len() >= MAX_OPERATOR_QUEUE {
            self.refusal(ordinal, "local command queue is full (8)")?;
            return Ok(());
        }
        let current_matches = self.current_frame.as_ref().is_some_and(|frame| {
            frame.sequence == u64::from(selected.view_sequence)
                && frame
                    .controller_views
                    .iter()
                    .filter(|v| v.address == selected.view.address)
                    .count()
                    == 1
                && frame.controller_views.contains(&selected.view)
        });
        if !current_matches {
            self.selected = None;
            self.clear_runtime_selection("STALE_COMMAND_REFUSAL")?;
            self.refusal(ordinal, "selected view is stale; SELECT again")?;
            return Ok(());
        }
        let command = make_command(selected, actor, intent, ordinal)?;
        operator_journal_push(
            &mut self.journal,
            format!(
                "QUEUE\t{ordinal}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{intent:?}\t{}\t{}\n",
                selected.view.address,
                selected.view.generation,
                selected.view_sequence,
                oa_sha256::to_hex(&selected.identity.gufi),
                oa_sha256::to_hex(&selected.identity.centre),
                oa_sha256::to_hex(&selected.identity.plan_id),
                actor,
                stamp.wall_ms,
                stamp.monotonic_ns
            ),
        )?;
        self.queue.push(command);
        self.update_overlay(&format!("QUEUED {}", self.queue.len()), ordinal, stamp)?;
        Ok(())
    }

    fn refusal(&mut self, ordinal: u64, reason: &str) -> Result<(), String> {
        operator_journal_push(
            &mut self.journal,
            format!(
                "REFUSAL\t{ordinal}\t{}\n",
                reason.replace(['\t', '\n'], " ")
            ),
        )?;
        if self.current_frame.is_some() {
            self.update_overlay(reason, ordinal, clock_stamp(self.wall_start)?)?;
        } else {
            writeln!(std::io::stdout(), "REFUSED: {reason}").map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn update_overlay(
        &mut self,
        text: &str,
        ordinal: u64,
        stamp: ClockStamp,
    ) -> Result<(), String> {
        self.reserve_presentation_update()?;
        let safe = text
            .chars()
            .take(128)
            .map(|c| {
                if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '?'
                }
            })
            .collect::<String>();
        let digest = crate::runtime::regional_console_overlay(&mut self.runtime, &safe)?;
        operator_journal_push(
            &mut self.journal,
            format!(
                "CONSOLE\t{ordinal}\t{}\t{}\t{}\t{}\n",
                stamp.wall_ms,
                stamp.monotonic_ns,
                safe,
                oa_sha256::to_hex(&digest)
            ),
        )?;
        Ok(())
    }

    fn reserve_presentation_update(&mut self) -> Result<(), String> {
        if self.console_redraws >= crate::surveillance::MAX_PRESENTATION_UPDATES {
            return Err("console presentation update budget exhausted".into());
        }
        self.console_redraws = self
            .console_redraws
            .checked_add(1)
            .ok_or("console redraw counter overflow")?;
        Ok(())
    }

    fn clear_runtime_selection(&mut self, reason: &str) -> Result<[u8; 32], String> {
        self.reserve_presentation_update()?;
        let digest = crate::runtime::regional_clear_selection(&mut self.runtime)?;
        operator_journal_push(
            &mut self.journal,
            format!(
                "CLEAR_SELECTION\t{reason}\t{}\n",
                oa_sha256::to_hex(&digest)
            ),
        )?;
        Ok(digest)
    }

    fn input_lost(&mut self, ordinal: u64, kind: &str, stamp: ClockStamp) -> Result<(), String> {
        let discarded = self.queue.len();
        self.queue.clear();
        self.input_loss_unresolved = true;
        self.selected = None;
        self.actor = None;
        let cutoff = self
            .current_frame
            .as_ref()
            .map_or(0, |frame| frame.surveillance.time_ns);
        operator_journal_push(
            &mut self.journal,
            format!(
                "INPUT_LOSS\t{ordinal}\t{kind}\t{discarded}\t{cutoff}\t{}\t{}\n",
                stamp.wall_ms, stamp.monotonic_ns
            ),
        )?;
        Ok(())
    }

    fn reject_input(&mut self, reason: &str, stamp: ClockStamp) -> Result<(), String> {
        if self.next_action >= MAX_SCRIPT_ACTIONS as u64 {
            return Err("operator session exceeds 4096 actions".into());
        }
        self.next_action = self
            .next_action
            .checked_add(1)
            .ok_or("operator action ordinal overflow")?;
        let ordinal = self.next_action;
        let cutoff = self
            .current_frame
            .as_ref()
            .map_or(0, |frame| frame.surveillance.time_ns);
        operator_journal_push(
            &mut self.journal,
            format!(
                "ACTION_REJECTED\t{ordinal}\t{}\t{}\t{cutoff}\t{reason}\n",
                stamp.wall_ms, stamp.monotonic_ns
            ),
        )?;
        self.refusal(ordinal, reason)
    }
}

fn init_runner(
    output: &Path,
    model_source: &crate::cli::ModelSource,
    batches: Vec<RecordedBatch>,
    audit_configuration: Option<OperatorAuditConfiguration>,
) -> Result<(SessionRunner, [u8; 32], [u8; 32]), String> {
    oa_sealed::refuse(output)?;
    let runway_audit = audit_configuration
        .map(|configuration| {
            let registry = oa_audit_log::shell::load_pinned_principal_registry(
                &configuration.registry_path,
                configuration.registry_sha256,
            )
            .map_err(|error| format!("pinned operator registry refused: {error}"))?;
            Ok::<RunwayAuditState, String>(RunwayAuditState {
                registry,
                audit_log: configuration.audit_log,
                checkpoint: configuration.checkpoint,
                principal_id: configuration.principal_id,
            })
        })
        .transpose()?;
    let cfg = crate::surveillance::SurveillanceConfig {
        centre_lat_e7: 410_000_000,
        centre_lon_e7: -870_000_000,
        range_nm: 180.0,
    };
    let (runtime, model_label, local_activation, processor_report) = match model_source {
        crate::cli::ModelSource::IssuedPackage {
            root,
            expected_manifest_sha256,
            processor_id,
        } => {
            let (package, activation) = match oa_adapt::local_inventory::verify_local_startup(
                root,
                *expected_manifest_sha256,
                *processor_id,
            ) {
                Ok(verified) => verified,
                Err(_) => {
                    fs::create_dir(output).map_err(|e| e.to_string())?;
                    let refusal = crate::processor_inventory::startup_refusal_report(
                        *processor_id,
                        *expected_manifest_sha256,
                        crate::processor_inventory::observed_unix_time_ns(),
                    );
                    write_processor_report(
                        &output.join("processor-report.tsv"),
                        refusal.as_bytes(),
                    )?;
                    return Err(
                        "issued adaptation startup verification refused; no model activated".into(),
                    );
                }
            };
            let runtime = match regional_new_from_issued_package(
                package,
                *b"ZAU1",
                cfg,
                crate::runtime::DEFAULT_REGIONAL_RUN_PROFILE,
            ) {
                Ok(runtime) => runtime,
                Err(_) => {
                    fs::create_dir(output).map_err(|e| e.to_string())?;
                    let refusal = crate::processor_inventory::startup_refusal_report(
                        *processor_id,
                        *expected_manifest_sha256,
                        crate::processor_inventory::observed_unix_time_ns(),
                    );
                    write_processor_report(
                        &output.join("processor-report.tsv"),
                        refusal.as_bytes(),
                    )?;
                    return Err(
                        "verified adaptation model did not activate; no runtime started".into(),
                    );
                }
            };
            let report = crate::processor_inventory::check_active_runtime_adaptation(
                &runtime,
                &activation,
                crate::processor_inventory::observed_unix_time_ns(),
            )
            .map_err(str::to_owned)?;
            if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
                fs::create_dir(output).map_err(|e| e.to_string())?;
                let refusal =
                    oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", &report);
                write_processor_report(&output.join("processor-report.tsv"), refusal.as_bytes())?;
                return Err("issued adaptation startup disk recheck refused; loaded package retained and run not activated".into());
            }
            let processor_report =
                oa_adapt::local_inventory::render_processor_report_tsv("STARTUP", &report);
            (
                runtime,
                "ISSUED_PACKAGE_PIN_VERIFIED",
                Some(activation),
                processor_report,
            )
        }
        _ => {
            let (model, model_sha, model_label) = load_model(model_source)?;
            let runtime = regional_new(model, model_sha, *b"ZAU1", cfg)?;
            (
                runtime,
                model_label,
                None,
                crate::processor_inventory::unpinned_source_report(),
            )
        }
    };
    let model_sha = regional_adaptation_sha256(&runtime);
    if let Some(activation) = local_activation.as_ref() {
        if model_sha != activation.loaded_adaptation_sha256() {
            return Err(
                "runtime adaptation fingerprint differs from verified startup model".into(),
            );
        }
    }
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable_sha =
        oa_sha256::sha256(&crate::cli::read_guarded(&current, MAX_EXECUTABLE_BYTES)?);
    fs::create_dir(output).map_err(|e| format!("create new output: {e}"))?;
    let settings = format!("OA_OPERATED_SETTINGS\t1\nmode\tRECORDED_REPLAY\nclock\trecorded receipt cutoffs; local input wall and monotonic time journaled\ncentre\tZAU1\nconfiguration\t{cfg:?}\ntracks\t32\nframes\t512\nmodel\t{model_label}\nnormalization\tpositive-address positions and explicit normalized state\n");
    let settings_sha256 = oa_sha256::sha256(settings.as_bytes());
    write_new(&output.join("settings.tsv"), settings.as_bytes())?;
    let start = Instant::now();
    Ok((
        SessionRunner {
            runtime,
            local_activation,
            processor_report,
            batches,
            next_batch: 0,
            next_sequence: [0; 5],
            next_action: 0,
            selected: None,
            actor: None,
            queue: Vec::with_capacity(MAX_OPERATOR_QUEUE),
            current_frame: None,
            journal: operator_journal_new(),
            frame_manifest: Vec::with_capacity(MAX_FRAMES),
            output: output.to_path_buf(),
            settings_sha256,
            console_redraws: 0,
            wall_start: start,
            input_loss_unresolved: false,
            runway_audit,
        },
        model_sha,
        executable_sha,
    ))
}

fn close_output(
    runner: &SessionRunner,
    input_sha: [u8; 32],
    model_sha: [u8; 32],
    executable_sha: [u8; 32],
    script_sha: [u8; 32],
) -> Result<(), String> {
    write_processor_report(
        &runner.output.join("processor-report.tsv"),
        runner.processor_report.as_bytes(),
    )?;
    write_new(
        &runner.output.join("operator.tsv"),
        operator_journal_bytes(&runner.journal),
    )?;
    let mut manifest = format!("OA_OPERATED_MANIFEST\t1\nmode\tRECORDED_REPLAY\ninput_sha256\t{}\nmodel_sha256\t{}\nexecutable_sha256\t{}\nsettings\tsettings.tsv\t{}\noperator_journal\toperator.tsv\t{}\nprocessor_report\tprocessor-report.tsv\t{}\nscript_sha256\t{}\n",
        oa_sha256::to_hex(&input_sha), oa_sha256::to_hex(&model_sha), oa_sha256::to_hex(&executable_sha),
        oa_sha256::to_hex(&runner.settings_sha256),
        oa_sha256::to_hex(&oa_sha256::sha256(operator_journal_bytes(&runner.journal))),
        oa_sha256::to_hex(&oa_sha256::sha256(runner.processor_report.as_bytes())),
        oa_sha256::to_hex(&script_sha));
    for row in &runner.frame_manifest {
        manifest.push_str(row);
    }
    write_new(&runner.output.join("manifest.tsv"), manifest.as_bytes())?;
    let directory = oa_sealed::open(&runner.output).map_err(|e| e.to_string())?;
    directory.sync_all().map_err(|e| e.to_string())?;
    let marker = runner.output.join("COMPLETE");
    let complete = format!(
        "{}\n",
        oa_sha256::to_hex(&oa_sha256::sha256(manifest.as_bytes()))
    );
    oa_sealed::refuse(&marker).map_err(|e| e.to_string())?;
    // seal-allow: create-new completion marker after path refusal; remove only this created file
    // if verification, write, fsync or parent-directory sync fails.
    let mut marker_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)
        .map_err(|e| format!("create completion marker: {e}"))?;
    let marker_result = oa_sealed::Sealed::production()
        .verify_open(&marker, &marker_file)
        .map_err(|e| e.to_string())
        .and_then(|_| {
            marker_file
                .write_all(complete.as_bytes())
                .map_err(|e| e.to_string())
        })
        .and_then(|_| marker_file.sync_all().map_err(|e| e.to_string()))
        .and_then(|_| directory.sync_all().map_err(|e| e.to_string()));
    drop(marker_file);
    if let Err(error) = marker_result {
        let _ = fs::remove_file(&marker);
        let _ = directory.sync_all();
        return Err(format!("could not durably close operated session: {error}"));
    }
    Ok(())
}

/// Performs one explicit package inventory check after operation has stopped advancing frames.
fn finish_processor_inventory(runner: &mut SessionRunner) -> Result<(), String> {
    if let Some(activation) = runner.local_activation.as_ref() {
        let report = crate::processor_inventory::check_active_runtime_adaptation(
            &runner.runtime,
            activation,
            crate::processor_inventory::observed_unix_time_ns(),
        )
        .map_err(str::to_owned)?;
        runner
            .processor_report
            .push_str(&oa_adapt::local_inventory::render_processor_report_tsv(
                "ON_DEMAND",
                &report,
            ));
        if report.status != oa_adapt::local_inventory::LocalInventoryStatus::Match {
            write_processor_report(
                &runner.output.join("processor-report.tsv"),
                runner.processor_report.as_bytes(),
            )?;
            return Err("issued adaptation on-demand verification found a mismatch; loaded model was retained and no completion marker was written".into());
        }
    }
    Ok(())
}

fn write_processor_report(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_PROCESSOR_REPORT_BYTES {
        return Err("processor report exceeds 1 MiB bound".into());
    }
    write_new(path, bytes)
}

/// Headless recorded-session runner. Logical frames and pixels are deterministic; receipt-time
/// action journal clocks intentionally make complete bundle bytes vary by run.
pub fn run_script_session(
    input: &Path,
    output: &Path,
    model_source: &crate::cli::ModelSource,
    script: &Path,
) -> Result<(), String> {
    run_script_session_inner(input, output, model_source, script, &[], None)
}

/// Normal headless host entry point for typed runway operator actions. Actions enter the same
/// selection, bounded queue, next-scan dispatch, journal and durable audit path as session input.
/// They are supplied as typed API values; this function adds no keyboard/script text grammar.
pub fn run_script_session_with_typed_operator_inputs(
    input: &Path,
    output: &Path,
    model_source: &crate::cli::ModelSource,
    script: &Path,
    inputs: &[TypedOperatorInput],
    audit: OperatorAuditConfiguration,
) -> Result<(), String> {
    run_script_session_inner(input, output, model_source, script, inputs, Some(audit))
}

fn run_script_session_inner(
    input: &Path,
    output: &Path,
    model_source: &crate::cli::ModelSource,
    script: &Path,
    inputs: &[TypedOperatorInput],
    audit_configuration: Option<OperatorAuditConfiguration>,
) -> Result<(), String> {
    let input_bytes = crate::cli::read_guarded(input, MAX_RECORDING_BYTES)?;
    let input_sha = oa_sha256::sha256(&input_bytes);
    let batches = parse_recording(&input_bytes)?;
    let script_bytes = crate::cli::read_guarded(script, MAX_SCRIPT_BYTES)?;
    let script_sha = oa_sha256::sha256(&script_bytes);
    let actions = parse_actions(&script_bytes)?;
    if inputs.len() > MAX_SCRIPT_ACTIONS {
        return Err("typed operator input count exceeds 4096".into());
    }
    let mut previous_action = 0u32;
    for input in inputs {
        if input.after_action == 0
            || input.after_action <= previous_action
            || usize::try_from(input.after_action).map_or(true, |index| index > actions.len())
            || !matches!(
                input.intent,
                Intent::AssignRunway(_) | Intent::AcknowledgeRunwayAlert
            )
        {
            return Err("typed operator inputs must be ordered runway actions after existing script actions".into());
        }
        previous_action = input.after_action;
    }
    let (mut runner, model_sha, executable_sha) =
        init_runner(output, model_source, batches, audit_configuration)?;
    let result = (|| {
        let mut input_index = 0usize;
        for (index, action) in actions.into_iter().enumerate() {
            let quit = runner.action(action, clock_stamp(runner.wall_start)?)?;
            let action_number =
                u32::try_from(index + 1).map_err(|_| "script action index overflow")?;
            while inputs
                .get(input_index)
                .is_some_and(|input| input.after_action == action_number)
            {
                let input = inputs.get(input_index).ok_or("typed input disappeared")?;
                runner.action(
                    Action::Intent(input.intent),
                    clock_stamp(runner.wall_start)?,
                )?;
                input_index = input_index
                    .checked_add(1)
                    .ok_or("typed input index overflow")?;
            }
            if quit {
                break;
            }
        }
        if input_index != inputs.len() {
            return Err("typed runway input was not consumed by the normal action loop".into());
        }
        if runner.current_frame.is_none() {
            return Err("operated session completed no frame".into());
        }
        if !runner.queue.is_empty() {
            return Err("operated session ends with queued commands".into());
        }
        if runner.input_loss_unresolved {
            return Err("operated session ends with unresolved input loss".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = write_new(
            &output.join("operator.tsv"),
            operator_journal_bytes(&runner.journal),
        );
        return Err(error);
    }
    finish_processor_inventory(&mut runner)?;
    close_output(&runner, input_sha, model_sha, executable_sha, script_sha)
}

fn present_native(
    display: &mut oa_display_present::KmsDisplay,
    runtime: &RegionalRuntime,
    buffer: &mut [u8],
    width: u32,
    height: u32,
) -> Result<(), String> {
    if display.lifecycle() != oa_display_present::KmsLifecycle::Active {
        return Ok(());
    }
    let pixels = regional_pixels(runtime)?;
    let row_bytes = usize::try_from(FRAME_WIDTH.checked_mul(4).ok_or("frame row overflow")?)
        .map_err(|_| "frame row width")?;
    let dst_stride = usize::try_from(width.checked_mul(4).ok_or("native row overflow")?)
        .map_err(|_| "native row width")?;
    let expected = row_bytes
        .checked_mul(FRAME_HEIGHT as usize)
        .ok_or("source frame overflow")?;
    if pixels.len() != expected {
        return Err("regional pixel buffer has unexpected size".into());
    }
    buffer.fill(0);
    let x = usize::try_from((width - FRAME_WIDTH) / 2)
        .map_err(|_| "native x offset")?
        .checked_mul(4)
        .ok_or("native x byte offset overflow")?;
    let y = usize::try_from((height - FRAME_HEIGHT) / 2).map_err(|_| "native y offset")?;
    for row in 0..FRAME_HEIGHT as usize {
        let src_start = row
            .checked_mul(row_bytes)
            .ok_or("source row offset overflow")?;
        let dst_start = (y + row)
            .checked_mul(dst_stride)
            .and_then(|v| v.checked_add(x))
            .ok_or("native row offset overflow")?;
        let src = pixels
            .get(src_start..src_start + row_bytes)
            .ok_or("source row outside frame")?;
        let dst = buffer
            .get_mut(dst_start..dst_start + row_bytes)
            .ok_or("native row outside buffer")?;
        dst.copy_from_slice(src);
    }
    display
        .present(oa_display_present::PixelFrame {
            pixels: buffer,
            width,
            height,
            stride: width * 4,
        })
        .map_err(|e| format!("KMS present: {e:?}"))
}

fn key_ascii(event: oa_display_present::InputEvent, shifted: bool) -> Option<char> {
    if event.kind != oa_display_present::EV_KEY {
        return None;
    }
    let code = event.code;
    let letter = match code {
        16 => 'Q',
        17 => 'W',
        18 => 'E',
        19 => 'R',
        20 => 'T',
        21 => 'Y',
        22 => 'U',
        23 => 'I',
        24 => 'O',
        25 => 'P',
        30 => 'A',
        31 => 'S',
        32 => 'D',
        33 => 'F',
        34 => 'G',
        35 => 'H',
        36 => 'J',
        37 => 'K',
        38 => 'L',
        44 => 'Z',
        45 => 'X',
        46 => 'C',
        47 => 'V',
        48 => 'B',
        49 => 'N',
        50 => 'M',
        _ => {
            return match code {
                57 => Some(' '),
                12 if shifted => Some('_'),
                12 => Some('-'),
                2 => Some('1'),
                3 => Some('2'),
                4 => Some('3'),
                5 => Some('4'),
                6 => Some('5'),
                7 => Some('6'),
                8 => Some('7'),
                9 => Some('8'),
                10 => Some('9'),
                11 => Some('0'),
                _ => None,
            }
        }
    };
    Some(letter)
}

fn usable_key_edge(event: oa_display_present::InputEvent, shift: bool) -> bool {
    event.kind == oa_display_present::EV_KEY
        && ((matches!(event.code, 42 | 54) && event.value == 0)
            || (oa_display_present::input_event_is_action_press(event)
                && (matches!(event.code, 1 | 14 | 28 | 42 | 54)
                    || key_ascii(event, shift).is_some())))
}

fn submit_line(
    runner: &mut SessionRunner,
    line: &mut String,
    stamp: ClockStamp,
) -> Result<bool, String> {
    let command = core::mem::take(line);
    let action = match parse_action(&command) {
        Ok(action) => action,
        Err(reason) => {
            runner.reject_input(&reason, stamp)?;
            return Ok(false);
        }
    };
    runner.action(action, stamp)
}

/// Native single-thread owner loop. Use only from a dedicated process that exits on every
/// returned error: a failed CRTC/VT restoration can require process termination.
pub fn run_device_session(
    input: &Path,
    output: &Path,
    model_source: &crate::cli::ModelSource,
    device: &NativeConsoleConfig<'_>,
) -> Result<(), String> {
    let NativeConsoleConfig {
        drm,
        tty,
        connector,
        input_device,
        idle_limit_seconds,
    } = *device;
    if idle_limit_seconds == 0 || idle_limit_seconds > NATIVE_IDLE_LIMIT_SECONDS {
        return Err("native idle limit must be 1..=300 seconds".into());
    }
    let input_bytes = crate::cli::read_guarded(input, MAX_RECORDING_BYTES)?;
    let input_sha = oa_sha256::sha256(&input_bytes);
    let batches = parse_recording(&input_bytes)?;
    let (mut runner, model_sha, executable_sha) = init_runner(output, model_source, batches, None)?;
    let mut display = oa_display_present::open_kms(drm, tty, connector)
        .map_err(|e| format!("KMS open: {e:?}"))?;
    // Include every post-acquisition initialization path in explicit cleanup; Drop is only
    // a fallback and cannot report a failed restore to the dedicated owner process.
    let operation = (|| {
        let mut evdev = oa_display_present::open_evdev(input_device)
            .map_err(|e| format!("evdev open: {e:?}"))?;
        let start = Instant::now();
        let initial_input = evdev
            .resynchronize(1024, 1000)
            .map_err(|e| format!("initial input resynchronization: {e:?}"))?;
        let initial_stamp = clock_stamp(start)?;
        operator_journal_push(
            &mut runner.journal,
            format!(
                "INPUT_RESYNC_INITIAL\t{}\t{}\t{}\t{}\n",
                initial_input.discarded_records,
                initial_input.relative_motion_lost,
                oa_sha256::to_hex(&input_snapshot_key_sha256(&initial_input)),
                initial_stamp.monotonic_ns
            ),
        )?;
        let (width, height) = display.dimensions();
        if width < FRAME_WIDTH || height < FRAME_HEIGHT {
            return Err("selected KMS mode is smaller than the fixed 800x600 replay view".into());
        }
        let bytes = usize::try_from(
            width
                .checked_mul(4)
                .and_then(|stride| stride.checked_mul(height))
                .ok_or("native buffer size overflow")?,
        )
        .map_err(|_| "native buffer size width")?;
        if bytes == 0
            || bytes > MAX_NATIVE_BUFFER_BYTES
            || bytes > oa_display_present::MAX_FRAME_BYTES
        {
            return Err("selected KMS mode exceeds the bounded native frame buffer".into());
        }
        let mut pixels = vec![0u8; bytes];
        display.start().map_err(|e| format!("KMS start: {e:?}"))?;
        // Start presents a zeroed frame. The regional view appears only after the first receipt.
        let mut last_input = Instant::now();
        let mut line = String::with_capacity(MAX_ACTION_BYTES);
        let mut shift = false;
        let mut quit = false;
        let mut was_active = true;
        let mut inactive_discarded = 0u32;
        let mut native_polls = 0u32;
        let result = (|| {
            while !quit {
                if native_polls >= MAX_NATIVE_POLLS
                    || start.elapsed().as_secs() >= MAX_NATIVE_SESSION_SECONDS
                {
                    return Err("native console session work or duration budget exhausted".into());
                }
                native_polls += 1;
                if last_input.elapsed().as_secs() >= idle_limit_seconds {
                    return Err("native console idle limit reached".into());
                }
                display
                    .service_vt(25)
                    .map_err(|e| format!("VT service: {e:?}"))?;
                if display.must_terminate_owner() {
                    return Err("KMS owner must terminate after a faulted VT transition".into());
                }
                let active = device_actions_enabled(display.lifecycle());
                if !active {
                    if was_active {
                        let discarded_commands = runner.queue.len();
                        let pending_bytes = line.len();
                        runner.queue.clear();
                        runner.selected = None;
                        runner.actor = None;
                        line.clear();
                        shift = false;
                        runner.input_loss_unresolved = true;
                        let clear_digest = if runner.current_frame.is_some() {
                            Some(runner.clear_runtime_selection("VT_RELEASE")?)
                        } else {
                            None
                        };
                        let stamp = clock_stamp(start)?;
                        let cutoff = runner
                            .current_frame
                            .as_ref()
                            .map_or(0, |frame| frame.surveillance.time_ns);
                        operator_journal_push(&mut runner.journal, format!("VT_RELEASE\t{discarded_commands}\t{pending_bytes}\t{cutoff}\t{}\t{}\t{}\n",
                        inactive_discarded, stamp.wall_ms, stamp.monotonic_ns))?;
                        if let Some(digest) = clear_digest {
                            operator_journal_push(
                                &mut runner.journal,
                                format!("VT_CLEAR_SELECTION\t{}\n", oa_sha256::to_hex(&digest)),
                            )?;
                        }
                        was_active = false;
                    }
                    match evdev.read_record(25) {
                        Ok(_) => {
                            inactive_discarded = inactive_discarded
                                .checked_add(1)
                                .ok_or("inactive input discard counter overflow")?
                        }
                        Err(error) if is_idle_input_timeout(&error) => {}
                        Err(error) => {
                            return Err(format!("evdev input while VT inactive: {error:?}"))
                        }
                    }
                    continue;
                }
                if !was_active {
                    let snapshot = evdev
                        .resynchronize(1024, 1000)
                        .map_err(|e| format!("VT reacquire input resynchronization: {e:?}"))?;
                    line.clear();
                    shift = false;
                    runner.input_loss_unresolved = false;
                    let stamp = clock_stamp(start)?;
                    let cutoff = runner
                        .current_frame
                        .as_ref()
                        .map_or(0, |frame| frame.surveillance.time_ns);
                    operator_journal_push(
                        &mut runner.journal,
                        format!(
                            "VT_REACQUIRE\t{inactive_discarded}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                            snapshot.discarded_records,
                            snapshot.relative_motion_lost,
                            oa_sha256::to_hex(&input_snapshot_key_sha256(&snapshot)),
                            cutoff,
                            stamp.wall_ms,
                            stamp.monotonic_ns
                        ),
                    )?;
                    inactive_discarded = 0;
                    was_active = true;
                    runner.update_overlay("VT REACQUIRED; READY", runner.next_action, stamp)?;
                    continue;
                }
                let input_poll = evdev.read_record(25);
                display
                    .service_vt(0)
                    .map_err(|e| format!("VT service before input dispatch: {e:?}"))?;
                if !device_actions_enabled(display.lifecycle()) {
                    if input_poll.is_ok() {
                        inactive_discarded = inactive_discarded
                            .checked_add(1)
                            .ok_or("inactive input discard counter overflow")?;
                    }
                    continue;
                }
                match input_poll {
                    Err(error) if is_idle_input_timeout(&error) => {}
                    Err(error) => {
                        runner.input_lost(
                            runner.next_action.saturating_add(1),
                            &format!("{error:?}"),
                            clock_stamp(start)?,
                        )?;
                        return Err(format!("evdev input lost: {error:?}"));
                    }
                    Ok(record) => match record.kind {
                        oa_display_present::InputRecordKind::SyncLost => {
                            line.clear();
                            shift = false;
                            runner.input_lost(
                                runner.next_action.saturating_add(1),
                                "SYN_DROPPED",
                                clock_stamp(start)?,
                            )?;
                            if runner.current_frame.is_some() {
                                runner.clear_runtime_selection("INPUT_LOSS")?;
                            }
                            let snapshot = evdev
                                .resynchronize(1024, 1000)
                                .map_err(|e| format!("input resynchronization: {e:?}"))?;
                            runner.input_loss_unresolved = false;
                            let stamp = clock_stamp(start)?;
                            let cutoff = runner
                                .current_frame
                                .as_ref()
                                .map_or(0, |frame| frame.surveillance.time_ns);
                            operator_journal_push(
                                &mut runner.journal,
                                format!(
                                    "INPUT_RESYNC\t{}\t{}\t{}\t{}\t{}\t{}\n",
                                    snapshot.discarded_records,
                                    snapshot.relative_motion_lost,
                                    oa_sha256::to_hex(&input_snapshot_key_sha256(&snapshot)),
                                    cutoff,
                                    stamp.wall_ms,
                                    stamp.monotonic_ns
                                ),
                            )?;
                            runner.update_overlay(
                                "INPUT RESYNCHRONIZED; READY",
                                runner.next_action,
                                stamp,
                            )?;
                        }
                        oa_display_present::InputRecordKind::Resynchronized => {
                            shift = false;
                            runner.input_loss_unresolved = false;
                            let stamp = clock_stamp(start)?;
                            let cutoff = runner
                                .current_frame
                                .as_ref()
                                .map_or(0, |frame| frame.surveillance.time_ns);
                            operator_journal_push(
                                &mut runner.journal,
                                format!(
                                    "INPUT_RESYNC\t{}\t{}\t{}\t{}\t{}\n",
                                    record.snapshot.discarded_records,
                                    record.snapshot.relative_motion_lost,
                                    cutoff,
                                    stamp.wall_ms,
                                    stamp.monotonic_ns
                                ),
                            )?;
                            runner.update_overlay(
                                "INPUT RESYNCHRONIZED",
                                runner.next_action,
                                clock_stamp(start)?,
                            )?;
                        }
                        oa_display_present::InputRecordKind::Event => {
                            // Relative motion, unmapped keys and kernel repeats cannot keep an
                            // unattended console alive. Only usable keyboard edges refresh idle.
                            let event = record.event;
                            if usable_key_edge(event, shift) {
                                last_input = Instant::now();
                            }
                            if runner.input_loss_unresolved {
                                operator_journal_push(
                                    &mut runner.journal,
                                    format!(
                                        "INPUT_IGNORED\t{}\t{}\t{}\n",
                                        runner.next_action.saturating_add(1),
                                        record.event.kind,
                                        record.event.code
                                    ),
                                )?;
                                continue;
                            }
                            if record.event.kind == oa_display_present::EV_KEY
                                && (record.event.code == 42 || record.event.code == 54)
                            {
                                if record.event.value == 1 {
                                    shift = true;
                                } else if record.event.value == 0 {
                                    shift = false;
                                }
                            } else if oa_display_present::input_event_is_action_press(record.event)
                            {
                                match record.event.code {
                                    1 => {
                                        line.clear();
                                        quit = runner.action(Action::Quit, clock_stamp(start)?)?;
                                    }
                                    28 => {
                                        quit = submit_line(
                                            &mut runner,
                                            &mut line,
                                            clock_stamp(start)?,
                                        )?;
                                    }
                                    14 => {
                                        line.pop();
                                    }
                                    _ => {
                                        if let Some(ch) = key_ascii(record.event, shift) {
                                            if line.len() < MAX_ACTION_BYTES {
                                                line.push(ch);
                                            } else {
                                                runner.reject_input(
                                                    "command line exceeds 128 ASCII bytes",
                                                    clock_stamp(start)?,
                                                )?;
                                                line.clear();
                                            }
                                        }
                                    }
                                }
                            }
                            if !line.is_empty() {
                                runner.update_overlay(
                                    &format!(">{line}"),
                                    runner.next_action,
                                    clock_stamp(start)?,
                                )?;
                            }
                        }
                        oa_display_present::InputRecordKind::Discarded => {
                            operator_journal_push(
                                &mut runner.journal,
                                format!(
                                    "INPUT_DISCARDED\t{}\t{}\n",
                                    runner.next_action.saturating_add(1),
                                    record.snapshot.discarded_records
                                ),
                            )?;
                        }
                        oa_display_present::InputRecordKind::SyncReport => {}
                    },
                }
                if runner.current_frame.is_some() {
                    present_native(&mut display, &runner.runtime, &mut pixels, width, height)?;
                }
            }
            if runner.current_frame.is_none() {
                return Err("operated session completed no frame".into());
            }
            if !runner.queue.is_empty() {
                return Err("QUIT with queued local commands is unresolved".into());
            }
            if runner.input_loss_unresolved {
                return Err("QUIT before evdev resynchronization is incomplete".into());
            }
            Ok(())
        })();
        result
    })();
    let close_result = display.close();
    if let Err(error) = operation {
        let _ = write_new(
            &output.join("operator.tsv"),
            operator_journal_bytes(&runner.journal),
        );
        return match close_result {
            Ok(()) => Err(error),
            Err(close_error) => Err(format!("{error}; KMS cleanup failed ({close_error:?}); dedicated owner process must terminate")),
        };
    }
    close_result
        .map_err(|e| format!("KMS close: {e:?}; dedicated owner process must terminate"))?;
    finish_processor_inventory(&mut runner)?;
    close_output(
        &runner,
        input_sha,
        model_sha,
        executable_sha,
        oa_sha256::sha256(b"NO_SCRIPT_DEVICE_MODE"),
    )
}

fn load_model(source: &crate::cli::ModelSource) -> Result<(Model, [u8; 32], &'static str), String> {
    let (bytes, label) = match source {
        crate::cli::ModelSource::Synthetic => (
            oa_airspace::core::snapshot::encode(&crate::cli::synthetic_model()?)
                .map_err(|e| format!("synthetic snapshot: {e:?}"))?,
            "SYNTHETIC",
        ),
        crate::cli::ModelSource::Snapshot(path) => (
            crate::cli::read_guarded(path, MAX_SNAPSHOT_BYTES)?,
            "SUPPLIED_SNAPSHOT",
        ),
        crate::cli::ModelSource::IssuedPackage { .. } => {
            return Err(
                "issued packages must be verified and activated as one runtime source".into(),
            )
        }
    };
    let model =
        oa_airspace::core::snapshot::decode(&bytes).map_err(|e| format!("snapshot: {e:?}"))?;
    Ok((model, oa_sha256::sha256(&bytes), label))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    oa_sealed::refuse(path)?;
    // seal-allow: create-new output after path refusal; verify the opened file before writing.
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    oa_sealed::Sealed::production()
        .verify_open(path, &file)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

fn save_frame(
    output: &Path,
    frame: &RegionalFrame,
    runtime: &RegionalRuntime,
) -> Result<([u8; 32], [u8; 32]), String> {
    let pixels = regional_pixels(runtime)?;
    let spec = oa_display::raster::FrameSpec {
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        stride: FRAME_WIDTH * 4,
    };
    let mut ppm = Vec::new();
    oa_display::ppm::write_ppm(&mut ppm, &spec, pixels).map_err(|e| format!("PPM: {e}"))?;
    let ppm_hash = oa_sha256::sha256(&ppm);
    let report = format!("OA_REGIONAL_OPERATED_FRAME\t1\npixels_ppm_sha256\t{}\nrendered_frame_sha256\t{}\n{frame:#?}\n",
        oa_sha256::to_hex(&ppm_hash), oa_sha256::to_hex(&frame.surveillance.frame_sha256));
    if report.len() > MAX_FRAME_REPORT_BYTES {
        return Err("frame report exceeds 512 KiB".into());
    }
    let report_hash = oa_sha256::sha256(report.as_bytes());
    write_new(
        &output.join(format!("frame-{:04}.txt", frame.sequence)),
        report.as_bytes(),
    )?;
    write_new(
        &output.join(format!("frame-{:04}.ppm", frame.sequence)),
        &ppm,
    )?;
    Ok((report_hash, ppm_hash))
}

#[cfg(test)]
mod native_state_tests {
    use super::{device_actions_enabled, is_idle_input_timeout, usable_key_edge};

    // REQ: OA-REG-016
    #[test]
    fn unrelated_motion_repeats_and_unmapped_keys_do_not_refresh_console_idle() {
        let press = oa_display_present::InputEvent {
            kind: oa_display_present::EV_KEY,
            code: 16,
            value: 1,
            ..oa_display_present::InputEvent::default()
        };
        assert!(usable_key_edge(press, false));
        assert!(!usable_key_edge(
            oa_display_present::InputEvent { value: 2, ..press },
            false
        ));
        assert!(!usable_key_edge(
            oa_display_present::InputEvent { code: 500, ..press },
            false
        ));
        assert!(!usable_key_edge(
            oa_display_present::InputEvent { kind: 2, ..press },
            false
        ));
        assert!(usable_key_edge(
            oa_display_present::InputEvent {
                code: 42,
                value: 0,
                ..press
            },
            false
        ));
    }

    // REQ: OA-REG-016
    #[test]
    fn idle_evdev_timeout_is_not_a_failure() {
        assert!(is_idle_input_timeout(
            &oa_display_present::InputDeviceError::Timeout
        ));
        assert!(!is_idle_input_timeout(
            &oa_display_present::InputDeviceError::Disconnected
        ));
    }

    // REQ: OA-REG-016
    #[test]
    fn only_the_active_vt_accepts_global_input_actions() {
        assert!(device_actions_enabled(
            oa_display_present::KmsLifecycle::Active
        ));
        assert!(!device_actions_enabled(
            oa_display_present::KmsLifecycle::Suspended
        ));
        assert!(!device_actions_enabled(
            oa_display_present::KmsLifecycle::Acquiring
        ));
        assert!(!device_actions_enabled(
            oa_display_present::KmsLifecycle::ReleasePending
        ));
    }
}

//! H2: bounded observed-state replay evidence. This is not an independent predictor oracle.
use std::io;

use oa_flight::{Alt, Op, Outcome, Stamp, F_COUNT, OP_COUNT, R_COUNT};

/// C7: at most 200 valid merged frames/s for a day, rounded upward. Caller may lower this.
pub const MAX_AUDIT_FRAMES: u64 = 18_000_000;
/// At most ten decoded rows/frame on average at the maximum frame budget; separately bounded.
pub const MAX_AUDIT_ROWS: u64 = 180_000_000;
/// Every row can emit ten operations plus a row record; frames also emit one record.
pub const MAX_AUDIT_EVENTS: u64 = 1_998_000_000;
/// Aggregate full on-disk input bytes (plain or compressed) reserved from actual opened files.
pub const MAX_AUDIT_RAW_INPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Offline budgets, all checked before work/emission; zero is not a valid budget.
// A7.2: cold configuration, size guard waived.
#[derive(Clone, Copy, Debug)]
pub struct AuditLimits {
    pub frames: u64,
    pub rows: u64,
    pub events: u64,
}

/// Source location, independent of recorder sequence resets and repeated timestamps.
// A7.2: cold evidence record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowIdentity {
    pub stream: u32,
    /// Offset continues across chained files in one stream; bound the input inventory separately.
    pub decoded_offset: u64,
    /// Recorder sequence is provenance only, not the unique key.
    pub recorder_sequence: u64,
    pub row: usize,
}

/// Decoded row presence and the adapter's disposition. Identifiers remain private local evidence.
// A7.2: cold evidence record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowObservation {
    pub identity: RowIdentity,
    pub source_time_ms: Option<i64>,
    pub received_time_ms: i64,
    pub gufi: Option<u128>,
    pub present: Option<u64>,
    pub field_errors: Option<u64>,
    pub operations: usize,
    pub centre: Option<[u8; 4]>,
    pub reported_owner: Option<[u8; 8]>,
    pub position_time_ms: Option<i64>,
    pub latitude_e7: Option<i32>,
    pub longitude_e7: Option<i32>,
    pub reported_altitude_ft: Option<i32>,
    pub no_status: bool,
    pub owner_unknown: bool,
    pub handoff_incomplete: bool,
    pub handoff_unknown_unit: bool,
    pub handoff_receiving_only: bool,
    pub departure_not_active: bool,
}

/// Resulting normalized state; absent flight/view remains absent, never filled with plausible data.
// A7.2: cold evidence record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateProjection {
    pub owner: [u8; 8],
    pub handoff_state: u8,
    pub handoff_from: [u8; 8],
    pub handoff_to: [u8; 8],
    pub accepted_by: [u8; 8],
    pub assigned: Alt,
    pub interim: Alt,
    pub requested: Alt,
    pub stamps: [Stamp; F_COUNT],
    pub plan_state: Option<u8>,
}

/// The operation carries source and receive clocks independently; neither is called controller time.
// A7.2: cold evidence record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationObservation {
    pub identity: RowIdentity,
    pub operation_index: usize,
    pub operation: Op,
    pub outcome: Outcome,
    pub state: Option<StateProjection>,
}

/// A frame's disposition, including non-decoded and sealed frames encountered by this shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDisposition {
    Decoded,
    BeforeWindow,
    AfterWindow,
    OtherFrame,
    XmlRefused,
    DecoderRefused(u8),
    SealedDocument,
}

/// Streamed records are observations of published state, not controller intent or prediction labels.
// A7.2: cold evidence record, size guard waived.
#[allow(clippy::large_enum_variant)] // C6: cold by-value record avoids a per-event heap allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Frame {
        identity: RowIdentity,
        received_ns: i64,
        disposition: FrameDisposition,
    },
    Row(RowObservation),
    Operation(OperationObservation),
}

/// Counts remain available to the caller on a cap, I/O error, or parser failure.
// A7.2: cold aggregate, size guard waived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditStats {
    pub frames_visited: u64,
    pub rows_delivered: u64,
    pub events_delivered: u64,
    pub rows_missing_key_or_time: u64,
    pub rows_without_operation: u64,
    pub operations_applied: [u64; OP_COUNT],
    pub operations_unchanged: [u64; OP_COUNT],
    pub operations_refused: [[u64; R_COUNT]; OP_COUNT],
    pub merge_files: usize,
    pub merge_bytes: u64,
    pub merge_errors: oa_replay::Errors,
    pub sealed_drops: u64,
    pub reader_steps_used: u64,
    pub decoded_bytes_charged: u64,
    /// Sum of full on-disk lengths reserved from the actual opened descriptors before reading.
    pub raw_input_bytes_reserved: u64,
    /// The 64 GiB aggregate raw/compressed input reservation was exceeded.
    pub raw_input_cap_refused: bool,
    pub cap_refused: bool,
    pub sink_failed: bool,
}

impl Default for AuditStats {
    fn default() -> Self {
        Self {
            frames_visited: 0,
            rows_delivered: 0,
            events_delivered: 0,
            rows_missing_key_or_time: 0,
            rows_without_operation: 0,
            operations_applied: [0; OP_COUNT],
            operations_unchanged: [0; OP_COUNT],
            operations_refused: [[0; R_COUNT]; OP_COUNT],
            merge_files: 0,
            merge_bytes: 0,
            merge_errors: [0; oa_replay::ERR_COUNT],
            sealed_drops: 0,
            reader_steps_used: 0,
            decoded_bytes_charged: 0,
            raw_input_bytes_reserved: 0,
            raw_input_cap_refused: false,
            cap_refused: false,
            sink_failed: false,
        }
    }
}

/// The shell owns its sink and buffers. Sink failure aborts replay; no dropped evidence is a pass.
// A7.2: cold shell state, size guard waived.
pub struct ReplayAudit<'a> {
    pub limits: AuditLimits,
    pub stats: AuditStats,
    pub inputs: Vec<oa_replay::source::ObservedInput>,
    pub sink: &'a mut dyn FnMut(&Observation) -> io::Result<()>,
}

pub fn audit_check_limits(limits: AuditLimits) -> io::Result<()> {
    if limits.frames == 0
        || limits.frames > MAX_AUDIT_FRAMES
        || limits.rows == 0
        || limits.rows > MAX_AUDIT_ROWS
        || limits.events == 0
        || limits.events > MAX_AUDIT_EVENTS
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "H2 audit limits out of range",
        ));
    }
    Ok(())
}

pub(crate) fn audit_frame(audit: &mut ReplayAudit<'_>) -> io::Result<()> {
    if audit.stats.frames_visited >= audit.limits.frames {
        audit.stats.cap_refused = true;
        return Err(io::Error::other(
            "H2 audit frame budget exhausted; INCOMPLETE",
        ));
    }
    audit.stats.frames_visited += 1;
    Ok(())
}

pub(crate) fn audit_emit(audit: &mut ReplayAudit<'_>, event: Observation) -> io::Result<()> {
    if audit.stats.events_delivered >= audit.limits.events
        || (matches!(event, Observation::Row(_)) && audit.stats.rows_delivered >= audit.limits.rows)
    {
        audit.stats.cap_refused = true;
        return Err(io::Error::other(
            "H2 audit row/event budget exhausted; INCOMPLETE",
        ));
    }

    // Validate the operation counter destination before the sink writes the event. Otherwise a
    // future operation/reason code could leave a serialized record that the terminal counters do
    // not account for.
    let operation_count = match event {
        Observation::Operation(op) => {
            let kind = usize::from(op.operation.kind);
            match op.outcome {
                Outcome::Applied if kind > 0 && kind < OP_COUNT => Some((kind, None)),
                Outcome::Unchanged if kind > 0 && kind < OP_COUNT => Some((kind, None)),
                Outcome::Refused(reason)
                    if kind > 0 && kind < OP_COUNT && usize::from(reason) < R_COUNT =>
                {
                    Some((kind, Some(usize::from(reason))))
                }
                _ => {
                    return Err(io::Error::other(
                        "H2 audit unknown normalized operation/outcome; INCOMPLETE",
                    ));
                }
            }
        }
        _ => None,
    };
    if let Err(error) = (audit.sink)(&event) {
        audit.stats.sink_failed = true;
        return Err(error);
    }
    audit.stats.events_delivered += 1;
    match event {
        Observation::Row(row) => {
            audit.stats.rows_delivered += 1;
            audit.stats.rows_missing_key_or_time +=
                u64::from(row.gufi.is_none() || row.source_time_ms.is_none());
            audit.stats.rows_without_operation += u64::from(row.operations == 0);
        }
        Observation::Operation(op) => match (op.outcome, operation_count) {
            (Outcome::Applied, Some((kind, _))) => {
                let count = audit
                    .stats
                    .operations_applied
                    .get_mut(kind)
                    .ok_or_else(|| {
                        io::Error::other("H2 audit operation counter invariant; INCOMPLETE")
                    })?;
                *count += 1;
            }
            (Outcome::Unchanged, Some((kind, _))) => {
                let count = audit
                    .stats
                    .operations_unchanged
                    .get_mut(kind)
                    .ok_or_else(|| {
                        io::Error::other("H2 audit operation counter invariant; INCOMPLETE")
                    })?;
                *count += 1;
            }
            (Outcome::Refused(_), Some((kind, Some(reason)))) => {
                let count = audit
                    .stats
                    .operations_refused
                    .get_mut(kind)
                    .and_then(|counts| counts.get_mut(reason))
                    .ok_or_else(|| {
                        io::Error::other("H2 audit operation counter invariant; INCOMPLETE")
                    })?;
                *count += 1;
            }
            _ => {
                return Err(io::Error::other(
                    "H2 audit operation counter invariant; INCOMPLETE",
                ))
            }
        },
        Observation::Frame { .. } => {}
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "none".into(), |value| value.to_string())
}

/// Stable H2-OBSERVED-STATE-1 record encoding: ASCII tab-separated explicit fields.
/// Codes are normalized engine/decoder codes; private GUIDs and unit bytes are hex.
pub fn observation_text(event: &Observation) -> String {
    use std::fmt::Write as _;
    let identity = match event {
        Observation::Frame { identity, .. } => *identity,
        Observation::Row(row) => row.identity,
        Observation::Operation(op) => op.identity,
    };
    let prefix = format!(
        "stream={}\toffset={}\tseq={}\trow={}",
        identity.stream, identity.decoded_offset, identity.recorder_sequence, identity.row
    );
    match event {
        Observation::Frame { received_ns, disposition, .. } => {
            let (code, reason) = match disposition {
                FrameDisposition::Decoded => (0,0), FrameDisposition::BeforeWindow => (1,0),
                FrameDisposition::AfterWindow => (2,0), FrameDisposition::OtherFrame => (3,0),
                FrameDisposition::XmlRefused => (4,0), FrameDisposition::DecoderRefused(reason) => (5,*reason),
                FrameDisposition::SealedDocument => (6,0),
            };
            format!("FRAME\t{prefix}\treceived_ns={received_ns}\tdisposition={code}\treason={reason}\n")
        }
        Observation::Row(row) => format!("ROW\t{prefix}\tsource_ms={}\treceived_ms={}\tgufi={}\tpresent={}\terrors={}\tops={}\tcentre={}\treported_owner={}\tposition_ms={}\tlat_e7={}\tlon_e7={}\treported_alt_ft={}\tno_status={}\towner_unknown={}\thandoff_incomplete={}\thandoff_unknown_unit={}\thandoff_receiving_only={}\tdeparture_not_active={}\n", optional(row.source_time_ms),row.received_time_ms,row.gufi.map_or_else(||"none".into(),|gufi|format!("{gufi:032x}")),optional(row.present),optional(row.field_errors),row.operations,row.centre.map_or_else(||"none".into(),|value|hex(&value)),row.reported_owner.map_or_else(||"none".into(),|value|hex(&value)),optional(row.position_time_ms),optional(row.latitude_e7),optional(row.longitude_e7),optional(row.reported_altitude_ft),u8::from(row.no_status),u8::from(row.owner_unknown),u8::from(row.handoff_incomplete),u8::from(row.handoff_unknown_unit),u8::from(row.handoff_receiving_only),u8::from(row.departure_not_active)),
        Observation::Operation(observed) => {
            let op = observed.operation;
            let (outcome, reason) = match observed.outcome { Outcome::Applied => (0,0), Outcome::Unchanged => (1,0), Outcome::Refused(reason) => (2,reason) };
            let mut out = format!("OP\t{prefix}\tindex={}\tkind={}\tgufi={:032x}\tcentre={}\tsource_ms={}\treceived_ms={}\tmessage={}\tstate={}\talt={},{},{}\tfix={}\tunit_a={}\tunit_b={}\tplan_id={}\taddress={}\tcallsign={}\tdeparture={}\tarrival={}\toutcome={outcome}\treason={reason}",observed.operation_index,op.kind,op.key.gufi,hex(&op.centre),op.ts_ms,op.seen_ms,op.msg,op.state,op.alt.form,op.alt.a_ft,op.alt.b_ft,hex(&op.fix),hex(&op.unit_a),hex(&op.unit_b),hex(&op.plan_id),op.address,hex(&op.callsign),hex(&op.dep),hex(&op.arr));
            match observed.state {
                None => out.push_str("\tflight=absent"),
                Some(state) => {
                    let _ = write!(out,"\tflight=present\towner={}\thandoff_state={}\thandoff_from={}\thandoff_to={}\taccepted_by={}\tassigned={},{},{}\tinterim={},{},{}\trequested={},{},{}\tplan_state={}",hex(&state.owner),state.handoff_state,hex(&state.handoff_from),hex(&state.handoff_to),hex(&state.accepted_by),state.assigned.form,state.assigned.a_ft,state.assigned.b_ft,state.interim.form,state.interim.a_ft,state.interim.b_ft,state.requested.form,state.requested.a_ft,state.requested.b_ft,optional(state.plan_state));
                    for (index,stamp) in state.stamps.iter().enumerate() { let _ = write!(out,"\tstamp{index}={},{},{}",stamp.ts_ms,stamp.avail_ms,stamp.msg); }
                }
            }
            out.push('\n');out
        }
    }
}

/// Build tokens are bounded canonical metadata; an invalid token cannot forge TSV records.
pub fn build_record(commit: Option<&str>, dirty: Option<&str>) -> io::Result<String> {
    let commit = match commit {
        None => "unknown",
        Some(value)
            if value.len() == 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            value
        }
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid embedded build commit",
            ))
        }
    };
    let dirty = match dirty {
        None => "unknown",
        Some("0") => "0",
        Some("1") => "1",
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid embedded build tree state",
            ))
        }
    };
    Ok(format!(
        "BUILD\tcommit={commit}\tdirty={dirty}\tbinary_identity=outer-build-manifest-required\n"
    ))
}

#[cfg(test)]
mod encoding_tests {
    use super::build_record;
    // REQ: OA-FLT-043
    #[test]
    fn build_tokens_cannot_inject_trace_records() {
        assert!(build_record(Some("abc\nROW\tforged=1"), Some("0")).is_err());
        assert!(build_record(
            Some("0000000000000000000000000000000000000000"),
            Some("0\nRESULT")
        )
        .is_err());
        assert!(build_record(None, None).is_ok());
        assert!(build_record(Some("0123456789abcdef0123456789abcdef01234567"), Some("0")).is_ok());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod counter_tests {
    use super::*;
    use oa_flight::{key_gufi, OP_OWNER};

    fn op(outcome: Outcome, kind: u8) -> Observation {
        Observation::Operation(OperationObservation {
            identity: RowIdentity {
                stream: 0,
                decoded_offset: 12,
                recorder_sequence: 3,
                row: 0,
            },
            operation_index: 0,
            operation: Op {
                kind,
                key: key_gufi(1),
                centre: [0; 4],
                ts_ms: 10,
                seen_ms: 11,
                msg: 12,
                state: 0,
                alt: Alt {
                    form: 0,
                    a_ft: 0,
                    b_ft: 0,
                },
                fix: [0; 12],
                unit_a: [0; 8],
                unit_b: [0; 8],
                plan_id: [0; 12],
                address: 0,
                callsign: [0; 8],
                dep: [0; 12],
                arr: [0; 12],
            },
            outcome,
            state: None,
        })
    }

    #[test]
    fn operation_counters_match_only_events_written_to_the_sink() {
        let written = std::cell::Cell::new(0usize);
        let mut sink = |_: &Observation| {
            written.set(written.get() + 1);
            Ok(())
        };
        let mut audit = ReplayAudit {
            limits: AuditLimits {
                frames: 2,
                rows: 2,
                events: 4,
            },
            stats: AuditStats::default(),
            inputs: Vec::new(),
            sink: &mut sink,
        };

        audit_emit(&mut audit, op(Outcome::Applied, OP_OWNER)).unwrap();
        audit_emit(&mut audit, op(Outcome::Unchanged, OP_OWNER)).unwrap();
        audit_emit(&mut audit, op(Outcome::Refused(3), OP_OWNER)).unwrap();

        assert_eq!(written.get(), 3);
        assert_eq!(audit.stats.events_delivered, 3);
        assert_eq!(
            audit.stats.operations_applied.get(usize::from(OP_OWNER)),
            Some(&1)
        );
        assert_eq!(
            audit.stats.operations_unchanged.get(usize::from(OP_OWNER)),
            Some(&1)
        );
        assert_eq!(
            audit
                .stats
                .operations_refused
                .get(usize::from(OP_OWNER))
                .and_then(|counts| counts.get(3)),
            Some(&1)
        );
    }

    #[test]
    fn unknown_operation_codes_are_refused_before_serialization() {
        let written = std::cell::Cell::new(0usize);
        let mut sink = |_: &Observation| {
            written.set(written.get() + 1);
            Ok(())
        };
        let mut audit = ReplayAudit {
            limits: AuditLimits {
                frames: 1,
                rows: 1,
                events: 1,
            },
            stats: AuditStats::default(),
            inputs: Vec::new(),
            sink: &mut sink,
        };

        assert!(audit_emit(&mut audit, op(Outcome::Refused(0), OP_COUNT as u8)).is_err());
        assert_eq!(written.get(), 0);
        assert_eq!(audit.stats.events_delivered, 0);

        assert!(audit_emit(&mut audit, op(Outcome::Applied, 0)).is_err());
        assert_eq!(written.get(), 0);
        assert_eq!(audit.stats.events_delivered, 0);
    }
}

//! Deterministic conversion of decoded recording rows into the normalized regional timeline.
//! Shell adapter (RULESET B1, B3, B7); callers own file decoding and receipt metadata.

use crate::input::{EventKind, Identity, NormalizedEvent, SourceKind};
use crate::recordings_reader::{RecordedFrame, RecordingCounts, RecordingSource};

use oa_adsb::{
    decode_payload_lenient_with_dropped, reports_len, reports_push, reports_row, Reports, F_GROUND,
    NONE_I32,
};
use oa_fixm::{new_events, Decoder, Field, Limits as FixmLimits};
use oa_flight::{
    Alt, ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS, OP_ADDRESS,
    OP_ASSIGN, OP_CANCEL, OP_COMPLETE, OP_COUNT, OP_DEPART, OP_DROP, OP_HO_ACCEPT, OP_HO_FAIL,
    OP_HO_INITIATE, OP_HO_RETRACT, OP_HO_TAKE, OP_HO_UPDATE, OP_IDENTITY, OP_INTERIM, OP_OWNER,
    OP_REQUEST, OP_SET_PLAN,
};
use oa_flight_sfdps::{row_ops, AdapterStats as SfdpsAdapterStats, MAX_OPS_PER_MESSAGE};
use oa_quarantine::{
    queue_new, route_input, Disposition, InputIdentity, InputOutcome, InputSource, QuarantineEvent,
    QuarantineQueue, ReasonFlags, SourceStats, REASON_ALTITUDE_ROUTE_INVALID,
    REASON_IDENTITY_OR_LOCATION_CONFLICT, REASON_INPUT_INVALID, REASON_RESOURCE_LIMIT,
    REASON_SURVEILLANCE_INVALID,
};
use oa_xml::{Limits as XmlLimits, Parser};
use std::time::Instant;

/// Stable source-order key supplied by the recording reader. It is preserved verbatim.
// A7.2: cold shell decoded row, retained only within bounded offline adaptation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedPosition {
    pub receipt_ns: i64,
    pub source_ns: i64,
    pub sequence: u64,
    pub address: u32,
    /// `Unique` is positive source evidence. `Unknown` and `Ambiguous` never receive a guessed ID.
    pub identity: RecordedIdentity,
    pub centre: [u8; 4],
    pub latitude_e7: i32,
    pub longitude_e7: i32,
    pub altitude_ft: Option<i32>,
    pub east_mmps: i32,
    pub north_mmps: i32,
    pub vertical_rate_fpm: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordedIdentity {
    Unknown,
    Unique { gufi: u128 },
    Ambiguous,
}

// A7.2: cold shell result counters for the position adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterCounts {
    pub examined: u64,
    pub emitted: u64,
    pub unknown_identity: u64,
    pub ambiguous_identity: u64,
    pub invalid_record: u64,
    pub invalid_time: u64,
    pub sealed_time: u64,
    pub invalid_address: u64,
    pub invalid_coordinate: u64,
    pub invalid_altitude: u64,
    pub invalid_centre: u64,
    pub over_capacity: u64,
}

// A7.2: cold shell adapter result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterOutput {
    /// Emitted rows remain in the exact input order; the adapter never sorts by either clock.
    pub events: Vec<NormalizedEvent>,
    pub counts: AdapterCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterError {
    ZeroCapacity,
    CountOverflow,
    UnsupportedEvent,
    InvalidCentre,
    InvalidField,
    RowTooLong,
    FramesNotMerged,
    WorkBudget,
    AllocationRefused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CutoffRefusal {
    Negative,
    Zero,
    SealedReceipt,
    Overflow,
    SealedCutoff,
}

const NS_PER_SECOND: i64 = 1_000_000_000;

/// Runtime issue time is the next whole-second cutoff at or after exact source receipt.
/// Rounding up is conservative: the normalized session never sees a frame before it arrived.
fn issue_cutoff(receipt_ns: i64) -> Result<i64, CutoffRefusal> {
    if receipt_ns < 0 {
        return Err(CutoffRefusal::Negative);
    }
    if receipt_ns == 0 {
        return Err(CutoffRefusal::Zero);
    }
    if oa_sealed::in_sealed_window(receipt_ns) {
        return Err(CutoffRefusal::SealedReceipt);
    }
    let seconds = receipt_ns / NS_PER_SECOND;
    let cutoff = if receipt_ns % NS_PER_SECOND == 0 {
        receipt_ns
    } else {
        seconds
            .checked_add(1)
            .and_then(|n| n.checked_mul(NS_PER_SECOND))
            .ok_or(CutoffRefusal::Overflow)?
    };
    if oa_sealed::in_sealed_window(cutoff) {
        return Err(CutoffRefusal::SealedCutoff);
    }
    Ok(cutoff)
}

/// Development sizing for the next adapter increment. The work cap is checked against decoded
/// rows before conversion; byte capacity is checked by the caller before retaining source rows.
// A7.2: cold shell limits for offline conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordingBudget {
    pub max_rows: usize,
    pub max_input_bytes: usize,
    pub max_work_items: usize,
}

/// 100k rows, 16 MiB on disk, bounded to one row conversion each. This is an offline development
/// profile aligned to the session input envelope, not a realtime or facility capacity claim.
pub const DEVELOPMENT_RECORDING_BUDGET: RecordingBudget = RecordingBudget {
    max_rows: 100_000,
    max_input_bytes: 16 * 1024 * 1024,
    max_work_items: 100_000,
};

/// Source decoder work caps for the 16 MiB OAF envelope. The ADS-B wire's shortest legacy row is
/// 64 bytes; FIXM rows are capped at the same 100k event budget as normalized output.
pub const MAX_ADSB_ROWS_PER_ADAPTATION: usize = 262_144;
pub const MAX_SFDPS_ROWS_PER_ADAPTATION: usize = 100_000;
/// Maximum manual-handling records: all bounded source rows plus one frame-level record per OAF
/// frame. This reserves a finite worst-case artifact instead of silently dropping overflow.
pub const MAX_QUARANTINE_RECORDS: usize = crate::recordings_reader::MAX_RECORDING_FRAMES
    + MAX_ADSB_ROWS_PER_ADAPTATION
    + MAX_SFDPS_ROWS_PER_ADAPTATION;

/// Worst-case retained normalized event storage for this profile, excluding allocator overhead.
/// This arithmetic is deliberately based on the actual inline event size, including route payload
/// space, rather than assuming every future input is a small position row.
pub fn event_storage_budget(budget: RecordingBudget) -> Result<usize, AdapterError> {
    core::mem::size_of::<NormalizedEvent>()
        .checked_mul(budget.max_rows)
        .ok_or(AdapterError::CountOverflow)
}

// A7.2: cold shell counters; not used in core hot-path storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedAdapterCounts {
    pub frames_adsb: u64,
    pub frames_sfdps: u64,
    pub nonmessage_frames_refused: u64,
    pub invalid_frame_metadata: u64,
    pub cutoff_negative_refused: u64,
    pub cutoff_zero_refused: u64,
    pub cutoff_sealed_receipt_refused: u64,
    pub cutoff_overflow_refused: u64,
    pub cutoff_sealed_issue_refused: u64,
    pub adsb_payload_refused: u64,
    pub adsb_rows_in: u64,
    pub adsb_rows_over_capacity: u64,
    pub adsb_rows_dropped: u64,
    pub adsb_ground_excluded: u64,
    pub adsb_unknown_ground_refused: u64,
    /// Decoded source flag byte frequencies, including excluded ground/unknown rows.
    pub adsb_native_flag_values: [u64; 256],
    /// Decoded source selector frequencies, including absent/unknown selectors.
    pub adsb_native_source_values: [u64; 256],
    pub adsb_source_time_regressions: u64,
    pub adsb_rows_order_refused: u64,
    pub adsb_velocity_unavailable: u64,
    /// Thirteen oa-truth quality refusal reasons, in the decoder's stable reason order.
    pub adsb_quality_removed: [u64; 13],
    pub adsb_unknown_gufi_positions: u64,
    pub sfdps_payload_refused: u64,
    pub sfdps_timestamp_precision_refused: u64,
    pub sfdps_rows_decoded: u64,
    pub sfdps_rows_not_decoded_capacity: u64,
    pub sfdps_frames_over_capacity: u64,
    pub sfdps_rows_refused: u64,
    pub sfdps_rows_no_operations: u64,
    pub sfdps_no_status: u64,
    pub sfdps_operations_unusable: u64,
    pub sfdps_owner_unknown: u64,
    pub sfdps_handoff_incomplete: u64,
    pub sfdps_handoff_unknown_unit: u64,
    pub sfdps_handoff_receiving_only: u64,
    pub sfdps_departure_not_active: u64,
    pub sfdps_not_decoded: u64,
    pub sfdps_sealed_messages: u64,
    pub sfdps_positions_unmapped: u64,
    pub sfdps_field_errors: u64,
    pub sfdps_rejected_messages: u64,
    pub sfdps_unknown_elements: u64,
    pub sfdps_unknown_attributes: u64,
    pub sfdps_unknown_overflow: u64,
    pub unsupported_operations: [u64; OP_COUNT],
    pub invalid_operation_time: u64,
    pub source_after_receipt: u64,
    pub adsb_source_after_receipt: u64,
    pub sfdps_source_after_receipt: u64,
    pub invalid_operation_identity: u64,
    pub invalid_operation_centre: u64,
    pub invalid_operation_address: u64,
    pub unbound_flight_updates: u64,
    pub invalid_position_rows: u64,
    pub output_capacity_refusals: u64,
    pub receipt_groups_over_capacity: u64,
    pub receipt_group_events_refused: u64,
    pub receipt_groups_over_frame_capacity: u64,
    pub receipt_frame_capacity_events_refused: u64,
    pub receipt_gap_groups_refused: u64,
    pub receipt_gap_events_refused: u64,
    pub unprocessed_frames: u64,
    pub events_emitted: u64,
}

pub(crate) fn zero_recorded_adapter_counts() -> RecordedAdapterCounts {
    RecordedAdapterCounts {
        frames_adsb: 0,
        frames_sfdps: 0,
        nonmessage_frames_refused: 0,
        invalid_frame_metadata: 0,
        cutoff_negative_refused: 0,
        cutoff_zero_refused: 0,
        cutoff_sealed_receipt_refused: 0,
        cutoff_overflow_refused: 0,
        cutoff_sealed_issue_refused: 0,
        adsb_payload_refused: 0,
        adsb_rows_in: 0,
        adsb_rows_over_capacity: 0,
        adsb_rows_dropped: 0,
        adsb_ground_excluded: 0,
        adsb_unknown_ground_refused: 0,
        adsb_native_flag_values: [0; 256],
        adsb_native_source_values: [0; 256],
        adsb_source_time_regressions: 0,
        adsb_rows_order_refused: 0,
        adsb_velocity_unavailable: 0,
        adsb_quality_removed: [0; 13],
        adsb_unknown_gufi_positions: 0,
        sfdps_payload_refused: 0,
        sfdps_timestamp_precision_refused: 0,
        sfdps_rows_decoded: 0,
        sfdps_rows_not_decoded_capacity: 0,
        sfdps_frames_over_capacity: 0,
        sfdps_rows_refused: 0,
        sfdps_rows_no_operations: 0,
        sfdps_no_status: 0,
        sfdps_operations_unusable: 0,
        sfdps_owner_unknown: 0,
        sfdps_handoff_incomplete: 0,
        sfdps_handoff_unknown_unit: 0,
        sfdps_handoff_receiving_only: 0,
        sfdps_departure_not_active: 0,
        sfdps_not_decoded: 0,
        sfdps_sealed_messages: 0,
        sfdps_positions_unmapped: 0,
        sfdps_field_errors: 0,
        sfdps_rejected_messages: 0,
        sfdps_unknown_elements: 0,
        sfdps_unknown_attributes: 0,
        sfdps_unknown_overflow: 0,
        unsupported_operations: [0; OP_COUNT],
        invalid_operation_time: 0,
        source_after_receipt: 0,
        adsb_source_after_receipt: 0,
        sfdps_source_after_receipt: 0,
        invalid_operation_identity: 0,
        invalid_operation_centre: 0,
        invalid_operation_address: 0,
        unbound_flight_updates: 0,
        invalid_position_rows: 0,
        output_capacity_refusals: 0,
        receipt_groups_over_capacity: 0,
        receipt_group_events_refused: 0,
        receipt_groups_over_frame_capacity: 0,
        receipt_frame_capacity_events_refused: 0,
        receipt_gap_groups_refused: 0,
        receipt_gap_events_refused: 0,
        unprocessed_frames: 0,
        events_emitted: 0,
    }
}

// A7.2: cold shell output retained within offline input and work bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedAdaptation {
    pub events: Vec<NormalizedEvent>,
    /// One-to-one audit trail for every normalized event, preserving the actual OAF source stream,
    /// frame sequence, frame byte offset, and within-frame emitted-item ordinal.
    pub provenance: Vec<EventProvenance>,
    pub counts: RecordedAdapterCounts,
    pub recording: RecordingCounts,
    /// Refused source items with exact source-local locators for later manual retrieval.
    pub quarantine: Vec<RecordedQuarantine>,
    /// Final queue counters, partitioned by flight data, surveillance, and adaptation.
    pub quarantine_stats: [SourceStats; oa_quarantine::INPUT_SOURCE_COUNT],
}

/// A manual-handling record linked to a bounded raw source item, never a copied payload.
// A7.2: cold offline output row capped by `MAX_QUARANTINE_RECORDS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedQuarantine {
    pub event: QuarantineEvent,
    pub stream_index: u8,
    pub frame_sequence: u64,
    pub frame_byte_offset: u64,
    /// Rows use source ordinal + 1; 131071 is reserved for an unlocatable frame-level refusal.
    pub row_locator: u32,
    /// True means the source-class queue was full and immediate manual handling is required.
    pub manual_handling_required: bool,
}

const QUARANTINE_FRAME_ROW_LOCATOR: u32 = 131_071;
const QUARANTINE_ITEM_SHIFT: u32 = 17;

fn zero_quarantine_stats() -> [SourceStats; oa_quarantine::INPUT_SOURCE_COUNT] {
    [SourceStats {
        accepted: 0,
        queued: 0,
        queue_full_manual_events: 0,
        acknowledged: 0,
        reason_counts: [0; oa_quarantine::REASON_COUNTER_COUNT],
    }; oa_quarantine::INPUT_SOURCE_COUNT]
}

fn quarantine_source(source: RecordingSource) -> InputSource {
    match source {
        RecordingSource::Adsb => InputSource::Surveillance,
        RecordingSource::Sfdps => InputSource::FlightData,
    }
}

/// Account one source item after bounded decoding/validation. This classifies completed work; it
/// does not interrupt a parser or provide a wall-time preemption guarantee.
// The source locator, bounded queue, work accounting, and offline clock fields form one decision.
#[allow(clippy::too_many_arguments)]
fn quarantine_item(
    queue: &mut QuarantineQueue,
    records: &mut Vec<RecordedQuarantine>,
    frame: &RecordedFrame,
    row_locator: u32,
    reasons: u16,
    work_units: u32,
    work_limit: u32,
    clock_origin: Instant,
    item_started: Instant,
) -> Result<bool, AdapterError> {
    let source_instance = u16::from(frame.stream_index)
        .checked_add(1)
        .ok_or(AdapterError::CountOverflow)?;
    if frame.byte_offset >= (1u64 << (64 - QUARANTINE_ITEM_SHIFT))
        || row_locator >= (1u32 << QUARANTINE_ITEM_SHIFT)
    {
        return Err(AdapterError::WorkBudget);
    }
    let item_id = frame
        .byte_offset
        .checked_shl(QUARANTINE_ITEM_SHIFT)
        .and_then(|base| base.checked_add(u64::from(row_locator)))
        .ok_or(AdapterError::CountOverflow)?;
    let started_ns = i64::try_from(
        item_started
            .checked_duration_since(clock_origin)
            .ok_or(AdapterError::WorkBudget)?
            .as_nanos(),
    )
    .map_err(|_| AdapterError::CountOverflow)?;
    let completed_ns = i64::try_from(clock_origin.elapsed().as_nanos())
        .map_err(|_| AdapterError::CountOverflow)?;
    let decision = route_input(
        queue,
        InputOutcome {
            identity: InputIdentity {
                source: quarantine_source(frame.source),
                source_instance,
                item_id,
            },
            refusal_reasons: ReasonFlags(reasons),
            clock_domain_id: 1,
            started_at_ns: started_ns,
            completed_at_ns: completed_ns,
            // Offline importer classification observes completion only; it never claims to
            // preempt parser work. The finite byte/row ceilings bound actual work before loops.
            elapsed_limit_ns: u64::MAX,
            work_units,
            work_limit,
        },
    )
    .map_err(|_| AdapterError::WorkBudget)?;
    match decision {
        Disposition::Accepted => Ok(true),
        Disposition::Queued(event) | Disposition::ManualHandlingRequired(event) => {
            if records.len() >= MAX_QUARANTINE_RECORDS {
                return Err(AdapterError::WorkBudget);
            }
            records
                .try_reserve(1)
                .map_err(|_| AdapterError::AllocationRefused)?;
            records.push(RecordedQuarantine {
                event,
                stream_index: frame.stream_index,
                frame_sequence: frame.sequence,
                frame_byte_offset: frame.byte_offset,
                row_locator,
                manual_handling_required: matches!(
                    decision,
                    Disposition::ManualHandlingRequired(_)
                ),
            });
            Ok(false)
        }
    }
}

fn provenance_row_locator(origin: EventProvenance) -> Result<u32, AdapterError> {
    let row = match origin.source {
        RecordingSource::Adsb => origin.emitted_item_ordinal,
        RecordingSource::Sfdps => origin.emitted_item_ordinal / (MAX_OPS_PER_MESSAGE as u32),
    };
    row.checked_add(1)
        .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
        .ok_or(AdapterError::WorkBudget)
}

fn quarantine_origin(
    queue: &mut QuarantineQueue,
    records: &mut Vec<RecordedQuarantine>,
    origin: EventProvenance,
    reasons: u16,
    work_units: u32,
    clock_origin: Instant,
) -> Result<bool, AdapterError> {
    let frame = RecordedFrame {
        source: origin.source,
        stream_index: origin.stream_index,
        receipt_ns: origin.input_receipt_ns,
        sequence: origin.frame_sequence,
        jms_timestamp_ms: origin.jms_timestamp_ms,
        mono_ns: origin.mono_ns,
        oaf_flags: origin.oaf_flags,
        byte_offset: origin.frame_byte_offset,
        skipped_before: origin.skipped_before,
        kind: oa_feed::Kind::Message,
        payload: Vec::new(),
    };
    quarantine_item(
        queue,
        records,
        &frame,
        provenance_row_locator(origin)?,
        reasons,
        work_units,
        if origin.source == RecordingSource::Sfdps {
            MAX_OPS_PER_MESSAGE as u32
        } else {
            1
        },
        clock_origin,
        clock_origin,
    )
}

fn account_provenance_range(
    queue: &mut QuarantineQueue,
    records: &mut Vec<RecordedQuarantine>,
    provenance: &[EventProvenance],
    range: core::ops::Range<usize>,
    reasons: u16,
    clock_origin: Instant,
) -> Result<(), AdapterError> {
    let mut index = range.start;
    while index < range.end {
        let origin = *provenance
            .get(index)
            .ok_or(AdapterError::UnsupportedEvent)?;
        let key = (
            origin.source,
            origin.stream_index,
            origin.frame_sequence,
            origin.frame_byte_offset,
            provenance_row_locator(origin)?,
        );
        let mut end = index.checked_add(1).ok_or(AdapterError::CountOverflow)?;
        while end < range.end {
            let next = *provenance.get(end).ok_or(AdapterError::UnsupportedEvent)?;
            let next_key = (
                next.source,
                next.stream_index,
                next.frame_sequence,
                next.frame_byte_offset,
                provenance_row_locator(next)?,
            );
            if next_key != key {
                break;
            }
            end = end.checked_add(1).ok_or(AdapterError::CountOverflow)?;
        }
        let accepted = quarantine_origin(
            queue,
            records,
            origin,
            reasons,
            u32::try_from(end - index).map_err(|_| AdapterError::WorkBudget)?,
            clock_origin,
        )?;
        if (reasons == 0) != accepted {
            return Err(AdapterError::WorkBudget);
        }
        index = end;
    }
    Ok(())
}

fn account_final_events(
    queue: &mut QuarantineQueue,
    records: &mut Vec<RecordedQuarantine>,
    provenance: &[EventProvenance],
    clock_origin: Instant,
) -> Result<(), AdapterError> {
    account_provenance_range(
        queue,
        records,
        provenance,
        0..provenance.len(),
        0,
        clock_origin,
    )
}

// A7.2: cold provenance row paired with one normalized event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventProvenance {
    pub source: RecordingSource,
    pub stream_index: u8,
    pub frame_sequence: u64,
    /// Exact OAF receipt before conservative issue-cutoff ceiling.
    pub input_receipt_ns: i64,
    pub jms_timestamp_ms: i64,
    pub mono_ns: Option<u64>,
    pub oaf_flags: u8,
    pub frame_byte_offset: u64,
    pub skipped_before: u64,
    pub emitted_item_ordinal: u32,
    /// Raw flags and selector from the decoded source row, retained without interpretation loss.
    pub native_flags: u32,
    pub native_source: [u8; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdsGroundClass {
    Ground,
    Airborne,
    Unknown,
}

/// readsb carries explicit ground state as `F_GROUND`; a numeric barometric altitude is the
/// available positive evidence of airborne status. Missing altitude without a ground bit stays
/// unknown and is refused by the AIRBORNE-only regional session.
fn adsb_ground_class(flags: u8, baro_alt_ft: i32) -> AdsGroundClass {
    if flags & F_GROUND != 0 {
        AdsGroundClass::Ground
    } else if baro_alt_ft != NONE_I32 {
        AdsGroundClass::Airborne
    } else {
        AdsGroundClass::Unknown
    }
}

/// Parses OAF messages, ceilings exact receipts to runtime issue cutoffs, then emits the complete
/// normalized timeline in cutoff/category/source order. ADS-B quality removals, unsupported source
/// precision and every unrepresentable operation are counted; no event fields are guessed.
pub fn adapt_recorded_frames(
    frames: &[RecordedFrame],
    recording_counts: RecordingCounts,
    max_events: usize,
    adsb_centre: [u8; 4],
) -> Result<RecordedAdaptation, AdapterError> {
    adapt_recorded_frames_with_sequence_starts(
        frames,
        recording_counts,
        max_events,
        adsb_centre,
        [0, 0],
    )
}

/// Bounded streaming counterpart for one ordered chunk. The caller carries the two sequence
/// starts across chunks and retains only one chunk's output. Default conversion continues to use
/// [`adapt_recorded_frames`] and its original behavior.
pub fn adapt_recorded_frames_with_sequence_starts(
    frames: &[RecordedFrame],
    recording_counts: RecordingCounts,
    max_events: usize,
    adsb_centre: [u8; 4],
    sequence_starts: [u64; 2],
) -> Result<RecordedAdaptation, AdapterError> {
    if max_events == 0 {
        return Err(AdapterError::ZeroCapacity);
    }
    if max_events > DEVELOPMENT_RECORDING_BUDGET.max_rows
        || frames.len() > crate::recordings_reader::MAX_RECORDING_FRAMES
    {
        return Err(AdapterError::WorkBudget);
    }
    if !adsb_centre.iter().all(u8::is_ascii_alphanumeric) {
        return Err(AdapterError::InvalidCentre);
    }
    let declared_decoded = usize::try_from(recording_counts.total_decoded_bytes)
        .map_err(|_| AdapterError::WorkBudget)?;
    if declared_decoded > DEVELOPMENT_RECORDING_BUDGET.max_input_bytes {
        return Err(AdapterError::WorkBudget);
    }
    let mut frame_payload_bytes = 0usize;
    for frame in frames {
        frame_payload_bytes = frame_payload_bytes
            .checked_add(frame.payload.len())
            .ok_or(AdapterError::WorkBudget)?;
    }
    if frame_payload_bytes > declared_decoded {
        return Err(AdapterError::WorkBudget);
    }
    for pair in frames.windows(2) {
        let [before, after] = pair else {
            return Err(AdapterError::FramesNotMerged);
        };
        let before_key = (
            before.receipt_ns,
            crate::recordings_reader::recording_source_rank(before.source),
            before.sequence,
            before.stream_index,
            before.byte_offset,
        );
        let after_key = (
            after.receipt_ns,
            crate::recordings_reader::recording_source_rank(after.source),
            after.sequence,
            after.stream_index,
            after.byte_offset,
        );
        if after_key < before_key {
            return Err(AdapterError::FramesNotMerged);
        }
    }
    let mut out = RecordedAdaptation {
        events: Vec::new(),
        provenance: Vec::new(),
        counts: zero_recorded_adapter_counts(),
        recording: recording_counts,
        quarantine: Vec::new(),
        quarantine_stats: zero_quarantine_stats(),
    };
    out.events
        .try_reserve_exact(max_events.min(4096))
        .map_err(|_| AdapterError::AllocationRefused)?;
    out.provenance
        .try_reserve_exact(max_events.min(4096))
        .map_err(|_| AdapterError::AllocationRefused)?;
    let mut parser = Parser::new(XmlLimits {
        max_input_bytes: oa_feed::MAX_PAYLOAD,
        max_elements: 262_144,
        max_attributes: 524_288,
        max_text_nodes: 262_144,
        ..XmlLimits::default()
    })
    .map_err(|_| AdapterError::UnsupportedEvent)?;
    let mut fixm_limits = FixmLimits::new(oa_sealed::sealed_drop_window());
    fixm_limits.max_elements = 262_144;
    let mut sfdps_decoder = Decoder::new(fixm_limits);
    let mut quarantine_queue = queue_new();
    let conversion_started = Instant::now();
    let mut next_surveillance_sequence = sequence_starts[0];
    let mut next_flight_sequence = sequence_starts[1];
    for frame in frames {
        let frame_started = Instant::now();
        if frame.kind != oa_feed::Kind::Message {
            out.counts.nonmessage_frames_refused =
                out.counts.nonmessage_frames_refused.saturating_add(1);
            quarantine_item(
                &mut quarantine_queue,
                &mut out.quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_INPUT_INVALID,
                u32::try_from(frame.payload.len()).unwrap_or(u32::MAX),
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        if frame.payload.len() > oa_feed::MAX_PAYLOAD {
            out.counts.invalid_frame_metadata = out.counts.invalid_frame_metadata.saturating_add(1);
            quarantine_item(
                &mut quarantine_queue,
                &mut out.quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_RESOURCE_LIMIT,
                u32::try_from(frame.payload.len()).unwrap_or(u32::MAX),
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        let issue_receipt_ns = match issue_cutoff(frame.receipt_ns) {
            Ok(value) => value,
            Err(CutoffRefusal::Negative) => {
                out.counts.cutoff_negative_refused =
                    out.counts.cutoff_negative_refused.saturating_add(1);
                quarantine_item(
                    &mut quarantine_queue,
                    &mut out.quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    frame.payload.len() as u32,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                continue;
            }
            Err(CutoffRefusal::Zero) => {
                out.counts.cutoff_zero_refused = out.counts.cutoff_zero_refused.saturating_add(1);
                quarantine_item(
                    &mut quarantine_queue,
                    &mut out.quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    frame.payload.len() as u32,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                continue;
            }
            Err(CutoffRefusal::SealedReceipt) => {
                out.counts.cutoff_sealed_receipt_refused =
                    out.counts.cutoff_sealed_receipt_refused.saturating_add(1);
                quarantine_item(
                    &mut quarantine_queue,
                    &mut out.quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    frame.payload.len() as u32,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                continue;
            }
            Err(CutoffRefusal::Overflow) => {
                out.counts.cutoff_overflow_refused =
                    out.counts.cutoff_overflow_refused.saturating_add(1);
                quarantine_item(
                    &mut quarantine_queue,
                    &mut out.quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    frame.payload.len() as u32,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                continue;
            }
            Err(CutoffRefusal::SealedCutoff) => {
                out.counts.cutoff_sealed_issue_refused =
                    out.counts.cutoff_sealed_issue_refused.saturating_add(1);
                quarantine_item(
                    &mut quarantine_queue,
                    &mut out.quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    frame.payload.len() as u32,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                continue;
            }
        };
        if out.events.len() >= max_events {
            out.counts.unprocessed_frames = out.counts.unprocessed_frames.saturating_add(1);
            quarantine_item(
                &mut quarantine_queue,
                &mut out.quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_RESOURCE_LIMIT,
                frame.payload.len() as u32,
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        match frame.source {
            RecordingSource::Adsb => {
                out.counts.frames_adsb = out.counts.frames_adsb.saturating_add(1);
                let mut quarantine = std::mem::take(&mut out.quarantine);
                let result = adapt_adsb_frame(
                    frame,
                    issue_receipt_ns,
                    adsb_centre,
                    max_events,
                    &mut next_surveillance_sequence,
                    &mut quarantine_queue,
                    &mut quarantine,
                    conversion_started,
                    frame_started,
                    &mut out,
                );
                out.quarantine = quarantine;
                result?;
            }
            RecordingSource::Sfdps => {
                out.counts.frames_sfdps = out.counts.frames_sfdps.saturating_add(1);
                let mut quarantine = std::mem::take(&mut out.quarantine);
                let result = adapt_sfdps_frame(
                    frame,
                    issue_receipt_ns,
                    max_events,
                    &mut next_flight_sequence,
                    &mut parser,
                    &mut sfdps_decoder,
                    &mut quarantine_queue,
                    &mut quarantine,
                    conversion_started,
                    frame_started,
                    &mut out,
                );
                out.quarantine = quarantine;
                result?;
            }
        }
    }
    sort_and_refuse_oversize_receipt_groups_with_quarantine(
        &mut out,
        &mut quarantine_queue,
        conversion_started,
    )?;
    out.counts.events_emitted =
        u64::try_from(out.events.len()).map_err(|_| AdapterError::CountOverflow)?;
    out.quarantine_stats = [
        oa_quarantine::source_stats(&quarantine_queue, InputSource::FlightData)
            .map_err(|_| AdapterError::WorkBudget)?,
        oa_quarantine::source_stats(&quarantine_queue, InputSource::Surveillance)
            .map_err(|_| AdapterError::WorkBudget)?,
        oa_quarantine::source_stats(&quarantine_queue, InputSource::Adaptation)
            .map_err(|_| AdapterError::WorkBudget)?,
    ];
    Ok(out)
}

// Keep source-specific state and accounting explicit at this narrow adapter boundary.
#[allow(clippy::too_many_arguments)]
fn adapt_adsb_frame(
    frame: &RecordedFrame,
    issue_receipt_ns: i64,
    adsb_centre: [u8; 4],
    max_events: usize,
    next_sequence: &mut u64,
    quarantine_queue: &mut QuarantineQueue,
    quarantine: &mut Vec<RecordedQuarantine>,
    conversion_started: Instant,
    frame_started: Instant,
    out: &mut RecordedAdaptation,
) -> Result<(), AdapterError> {
    let record_count = match oa_adsb::payload_record_count(&frame.payload) {
        Ok(count) => count,
        Err(_) => {
            out.counts.adsb_payload_refused = out.counts.adsb_payload_refused.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_INPUT_INVALID,
                u32::try_from(frame.payload.len()).map_err(|_| AdapterError::WorkBudget)?,
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            return Ok(());
        }
    };
    let previous_rows =
        usize::try_from(out.counts.adsb_rows_in).map_err(|_| AdapterError::WorkBudget)?;
    let admitted_rows = previous_rows
        .checked_add(record_count)
        .ok_or(AdapterError::WorkBudget)?;
    out.counts.adsb_rows_in =
        u64::try_from(admitted_rows).map_err(|_| AdapterError::CountOverflow)?;
    if admitted_rows > MAX_ADSB_ROWS_PER_ADAPTATION {
        out.counts.adsb_rows_over_capacity = out
            .counts
            .adsb_rows_over_capacity
            .saturating_add(u64::try_from(record_count).map_err(|_| AdapterError::CountOverflow)?);
        for ordinal in 0..record_count {
            let row_locator = u32::try_from(ordinal)
                .ok()
                .and_then(|value| value.checked_add(1))
                .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
                .ok_or(AdapterError::WorkBudget)?;
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_RESOURCE_LIMIT,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
        }
        return Ok(());
    }
    let mut reports = Reports::default();
    let mut dropped_ordinals = Vec::new();
    dropped_ordinals
        .try_reserve(record_count)
        .map_err(|_| AdapterError::AllocationRefused)?;
    let counts =
        match decode_payload_lenient_with_dropped(&frame.payload, &mut reports, |ordinal| {
            dropped_ordinals.push(ordinal)
        }) {
            Ok(counts) => counts,
            Err(_) => {
                out.counts.adsb_payload_refused = out.counts.adsb_payload_refused.saturating_add(1);
                quarantine_item(
                    quarantine_queue,
                    quarantine,
                    frame,
                    QUARANTINE_FRAME_ROW_LOCATOR,
                    REASON_INPUT_INVALID,
                    u32::try_from(frame.payload.len()).map_err(|_| AdapterError::WorkBudget)?,
                    oa_feed::MAX_PAYLOAD as u32,
                    conversion_started,
                    frame_started,
                )?;
                return Ok(());
            }
        };
    if counts.records != record_count {
        return Err(AdapterError::WorkBudget);
    }
    out.counts.adsb_rows_dropped = out
        .counts
        .adsb_rows_dropped
        .saturating_add(u64::try_from(counts.dropped).map_err(|_| AdapterError::CountOverflow)?);
    let mut decoded_source_ordinals = Vec::new();
    decoded_source_ordinals
        .try_reserve_exact(counts.kept)
        .map_err(|_| AdapterError::AllocationRefused)?;
    let mut drop_cursor = 0usize;
    for ordinal in 0..counts.records {
        if dropped_ordinals.get(drop_cursor) == Some(&ordinal) {
            drop_cursor += 1;
        } else {
            decoded_source_ordinals.push(ordinal);
        }
    }
    for &ordinal in &dropped_ordinals {
        let row_locator = u32::try_from(ordinal)
            .ok()
            .and_then(|value| value.checked_add(1))
            .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
            .ok_or(AdapterError::WorkBudget)?;
        quarantine_item(
            quarantine_queue,
            quarantine,
            frame,
            row_locator,
            REASON_SURVEILLANCE_INVALID,
            1,
            1,
            conversion_started,
            frame_started,
        )?;
    }
    if reports
        .time_ns
        .windows(2)
        .any(|pair| matches!(pair, [before, after] if after < before))
    {
        out.counts.adsb_source_time_regressions =
            out.counts.adsb_source_time_regressions.saturating_add(1);
        out.counts.adsb_rows_order_refused = out
            .counts
            .adsb_rows_order_refused
            .saturating_add(u64::try_from(counts.kept).map_err(|_| AdapterError::CountOverflow)?);
        for &ordinal in &decoded_source_ordinals {
            let row_locator = u32::try_from(ordinal)
                .ok()
                .and_then(|value| value.checked_add(1))
                .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
                .ok_or(AdapterError::WorkBudget)?;
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_INPUT_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
        }
        return Ok(());
    }
    let (airborne_reports, airborne_report_indexes, unknown_ground_indexes) =
        airborne_adsb_reports(&reports, &mut out.counts)?;
    let mut airborne_source_ordinals = Vec::new();
    airborne_source_ordinals
        .try_reserve_exact(airborne_report_indexes.len())
        .map_err(|_| AdapterError::AllocationRefused)?;
    for report_index in airborne_report_indexes {
        airborne_source_ordinals.push(
            *decoded_source_ordinals
                .get(report_index)
                .ok_or(AdapterError::WorkBudget)?,
        );
    }
    for report_index in unknown_ground_indexes {
        let ordinal = *decoded_source_ordinals
            .get(report_index)
            .ok_or(AdapterError::WorkBudget)?;
        let row_locator = u32::try_from(ordinal)
            .ok()
            .and_then(|value| value.checked_add(1))
            .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
            .ok_or(AdapterError::WorkBudget)?;
        quarantine_item(
            quarantine_queue,
            quarantine,
            frame,
            row_locator,
            REASON_SURVEILLANCE_INVALID,
            1,
            1,
            conversion_started,
            frame_started,
        )?;
    }
    // Reuse the tracker’s existing quality/velocity conversion so the regional adapter does not
    // duplicate ADS-B unit math or platform trig. All quality-gated row losses remain reported.
    let (measurements, stats) =
        oa_track::adsb::adsb_measurements(&airborne_reports, &oa_track::adsb::DEFAULT_ADSB_CONFIG);
    for (dst, src) in out
        .counts
        .adsb_quality_removed
        .iter_mut()
        .zip(stats.removed)
    {
        *dst = dst.saturating_add(src);
    }
    let mut source_row_cursor = 0usize;
    let mut represented_rows = Vec::new();
    represented_rows
        .try_reserve_exact(airborne_source_ordinals.len())
        .map_err(|_| AdapterError::AllocationRefused)?;
    represented_rows.resize(airborne_source_ordinals.len(), false);
    for measurement in measurements {
        let mut source_row = None;
        let mut source_index = None;
        while source_row_cursor < reports_len(&airborne_reports) {
            let index = source_row_cursor;
            source_row_cursor += 1;
            let Some(candidate) = reports_row(&airborne_reports, index) else {
                continue;
            };
            if candidate.icao == measurement.addr
                && candidate.time_ns == measurement.toa_ns
                && candidate.lat_e7 == measurement.lat_e7
                && candidate.lon_e7 == measurement.lon_e7
            {
                source_row = Some(candidate);
                source_index = Some(index);
                break;
            }
        }
        let Some(source_row) = source_row else {
            out.counts.invalid_position_rows = out.counts.invalid_position_rows.saturating_add(1);
            continue;
        };
        let source_index = source_index.ok_or(AdapterError::WorkBudget)?;
        let represented = represented_rows
            .get_mut(source_index)
            .ok_or(AdapterError::WorkBudget)?;
        *represented = true;
        let source_row_ordinal = airborne_source_ordinals
            .get(source_index)
            .copied()
            .ok_or(AdapterError::WorkBudget)?;
        let row_locator = u32::try_from(source_row_ordinal)
            .ok()
            .and_then(|value| value.checked_add(1))
            .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
            .ok_or(AdapterError::WorkBudget)?;
        if out.events.len() >= max_events {
            out.counts.output_capacity_refusals =
                out.counts.output_capacity_refusals.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_RESOURCE_LIMIT,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        if measurement.flags & oa_track::M_HAS_VEL == 0 {
            out.counts.adsb_velocity_unavailable =
                out.counts.adsb_velocity_unavailable.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_SURVEILLANCE_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        if measurement.toa_ns > frame.receipt_ns {
            out.counts.source_after_receipt = out.counts.source_after_receipt.saturating_add(1);
            out.counts.adsb_source_after_receipt =
                out.counts.adsb_source_after_receipt.saturating_add(1);
            out.counts.invalid_position_rows = out.counts.invalid_position_rows.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_INPUT_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        let Some(east_mmps) = fixed_mmps(measurement.vel_east_mps) else {
            out.counts.invalid_position_rows = out.counts.invalid_position_rows.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_SURVEILLANCE_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
            continue;
        };
        let Some(north_mmps) = fixed_mmps(measurement.vel_north_mps) else {
            out.counts.invalid_position_rows = out.counts.invalid_position_rows.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_SURVEILLANCE_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
            continue;
        };
        let altitude_ft = (measurement.alt_ft != oa_track::NONE_I32).then_some(measurement.alt_ft);
        let vertical_rate_fpm =
            (measurement.vrate_fpm != oa_track::NONE_I32).then_some(measurement.vrate_fpm);
        let position = RecordedPosition {
            receipt_ns: issue_receipt_ns,
            source_ns: measurement.toa_ns,
            sequence: *next_sequence,
            address: measurement.addr,
            identity: RecordedIdentity::Unknown,
            centre: adsb_centre,
            latitude_e7: measurement.lat_e7,
            longitude_e7: measurement.lon_e7,
            altitude_ft,
            east_mmps,
            north_mmps,
            vertical_rate_fpm,
        };
        let adapted = adapt_positions(core::slice::from_ref(&position), 1)?;
        out.counts.adsb_unknown_gufi_positions = out
            .counts
            .adsb_unknown_gufi_positions
            .saturating_add(adapted.counts.unknown_identity);
        if let Some(event) = adapted.events.into_iter().next() {
            let next = next_sequence
                .checked_add(1)
                .ok_or(AdapterError::CountOverflow)?;
            out.events
                .try_reserve(1)
                .map_err(|_| AdapterError::AllocationRefused)?;
            out.provenance
                .try_reserve(1)
                .map_err(|_| AdapterError::AllocationRefused)?;
            out.events.push(event);
            out.provenance.push(EventProvenance {
                source: frame.source,
                stream_index: frame.stream_index,
                frame_sequence: frame.sequence,
                input_receipt_ns: frame.receipt_ns,
                jms_timestamp_ms: frame.jms_timestamp_ms,
                mono_ns: frame.mono_ns,
                oaf_flags: frame.oaf_flags,
                frame_byte_offset: frame.byte_offset,
                skipped_before: frame.skipped_before,
                emitted_item_ordinal: u32::try_from(source_row_ordinal)
                    .map_err(|_| AdapterError::CountOverflow)?,
                native_flags: u32::from(source_row.flags),
                native_source: [source_row.source, 0],
            });
            *next_sequence = next;
        } else {
            out.counts.invalid_position_rows = out.counts.invalid_position_rows.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_SURVEILLANCE_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
        }
    }
    for (index, represented) in represented_rows.into_iter().enumerate() {
        if !represented {
            let source_ordinal = airborne_source_ordinals
                .get(index)
                .copied()
                .ok_or(AdapterError::WorkBudget)?;
            let row_locator = u32::try_from(source_ordinal)
                .ok()
                .and_then(|value| value.checked_add(1))
                .filter(|value| *value < QUARANTINE_FRAME_ROW_LOCATOR)
                .ok_or(AdapterError::WorkBudget)?;
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                row_locator,
                REASON_SURVEILLANCE_INVALID,
                1,
                1,
                conversion_started,
                frame_started,
            )?;
        }
    }
    Ok(())
}

fn airborne_adsb_reports(
    reports: &Reports,
    counts: &mut RecordedAdapterCounts,
) -> Result<(Reports, Vec<usize>, Vec<usize>), AdapterError> {
    let mut airborne_reports = Reports::default();
    let mut source_ordinals = Vec::new();
    let mut unknown_ground_ordinals = Vec::new();
    airborne_reports
        .icao
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    airborne_reports
        .time_ns
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    airborne_reports
        .lat_e7
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    airborne_reports
        .lon_e7
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    source_ordinals
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    unknown_ground_ordinals
        .try_reserve(reports_len(reports))
        .map_err(|_| AdapterError::AllocationRefused)?;
    for i in 0..reports_len(reports) {
        let Some(row) = reports_row(reports, i) else {
            counts.invalid_position_rows = counts.invalid_position_rows.saturating_add(1);
            continue;
        };
        if let Some(count) = counts
            .adsb_native_flag_values
            .get_mut(usize::from(row.flags))
        {
            *count = count.saturating_add(1);
        }
        if let Some(count) = counts
            .adsb_native_source_values
            .get_mut(usize::from(row.source))
        {
            *count = count.saturating_add(1);
        }
        match adsb_ground_class(row.flags, row.baro_alt_ft) {
            AdsGroundClass::Ground => {
                counts.adsb_ground_excluded = counts.adsb_ground_excluded.saturating_add(1)
            }
            AdsGroundClass::Unknown => {
                counts.adsb_unknown_ground_refused =
                    counts.adsb_unknown_ground_refused.saturating_add(1);
                unknown_ground_ordinals.push(i);
            }
            AdsGroundClass::Airborne => {
                reports_push(&mut airborne_reports, &row);
                source_ordinals.push(i);
            }
        }
    }
    Ok((airborne_reports, source_ordinals, unknown_ground_ordinals))
}

/// Refuses any decoded FIXM timestamp whose discarded sub-millisecond digits carry information.
/// `oa-fixm` intentionally returns Unix milliseconds; this guard prevents its documented
/// fractional truncation from becoming a silent source-clock repair at this boundary.
fn has_submillisecond_flight_timestamp(doc: &oa_xml::Doc<'_>, input: &[u8]) -> bool {
    for (element, &name_span) in doc.elem_name.iter().enumerate() {
        if oa_xml::span_bytes(input, name_span).ok() != Some(b"flight".as_slice()) {
            continue;
        }
        let Some(&first) = doc.elem_first_attr.get(element) else {
            continue;
        };
        let Some(&count) = doc.elem_attr_count.get(element) else {
            continue;
        };
        let first = first.0 as usize;
        let end = first
            .saturating_add(count as usize)
            .min(doc.attr_name.len());
        for index in first..end {
            if doc
                .attr_prefix
                .get(index)
                .is_some_and(|prefix| prefix.len != 0)
            {
                continue;
            }
            let Some(&attr_name) = doc.attr_name.get(index) else {
                continue;
            };
            if oa_xml::span_bytes(input, attr_name).ok() != Some(b"timestamp".as_slice()) {
                continue;
            }
            let Some(&value_span) = doc.attr_value.get(index) else {
                continue;
            };
            let mut value = [0u8; 64];
            let Ok(len) =
                oa_xml::decode_into(value_span, input, &mut value, oa_xml::SpanKind::AttrValue)
            else {
                continue;
            };
            let Some(bytes) = value.get(..len) else {
                continue;
            };
            let Some(dot) = bytes.iter().position(|&byte| byte == b'.') else {
                continue;
            };
            let Some(z) = bytes.iter().position(|&byte| byte == b'Z') else {
                continue;
            };
            if z > dot + 4
                && bytes
                    .get(dot + 4..z)
                    .is_some_and(|fraction| fraction.iter().any(|&digit| digit != b'0'))
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
fn sort_and_refuse_oversize_receipt_groups(
    out: &mut RecordedAdaptation,
) -> Result<(), AdapterError> {
    let mut queue = queue_new();
    sort_and_refuse_oversize_receipt_groups_with_quarantine(out, &mut queue, Instant::now())
}

fn sort_and_refuse_oversize_receipt_groups_with_quarantine(
    out: &mut RecordedAdaptation,
    quarantine_queue: &mut QuarantineQueue,
    clock_origin: Instant,
) -> Result<(), AdapterError> {
    use crate::session::MAX_EVENTS_PER_RECEIPT;
    use crate::surveillance::{MAX_FRAMES, MAX_FRAME_GAP_NS};
    if out.events.len() != out.provenance.len() {
        return Err(AdapterError::UnsupportedEvent);
    }
    let mut order = Vec::new();
    order
        .try_reserve_exact(out.events.len())
        .map_err(|_| AdapterError::AllocationRefused)?;
    order.extend(0..out.events.len());
    let key = |index: usize| match (out.events.get(index), out.provenance.get(index)) {
        (Some(event), Some(provenance)) => (
            event.receipt_ns,
            event.source,
            provenance.input_receipt_ns,
            provenance.frame_sequence,
            provenance.stream_index,
            provenance.frame_byte_offset,
            provenance.emitted_item_ordinal,
        ),
        _ => (
            i64::MAX,
            SourceKind::Adaptation,
            i64::MAX,
            u64::MAX,
            u8::MAX,
            u64::MAX,
            u32::MAX,
        ),
    };
    order.sort_unstable_by_key(|index| key(*index));
    let event_len = out.events.len();
    for i in 0..event_len {
        let Some(&mapped) = order.get(i) else {
            return Err(AdapterError::UnsupportedEvent);
        };
        if mapped == i {
            continue;
        }
        let saved_event = *out.events.get(i).ok_or(AdapterError::UnsupportedEvent)?;
        let saved_provenance = *out
            .provenance
            .get(i)
            .ok_or(AdapterError::UnsupportedEvent)?;
        let mut current = i;
        loop {
            let next = *order.get(current).ok_or(AdapterError::UnsupportedEvent)?;
            if next >= event_len {
                return Err(AdapterError::UnsupportedEvent);
            }
            if next == i {
                *out.events
                    .get_mut(current)
                    .ok_or(AdapterError::UnsupportedEvent)? = saved_event;
                *out.provenance
                    .get_mut(current)
                    .ok_or(AdapterError::UnsupportedEvent)? = saved_provenance;
                *order
                    .get_mut(current)
                    .ok_or(AdapterError::UnsupportedEvent)? = current;
                break;
            }
            let next_event = *out.events.get(next).ok_or(AdapterError::UnsupportedEvent)?;
            let next_provenance = *out
                .provenance
                .get(next)
                .ok_or(AdapterError::UnsupportedEvent)?;
            *out.events
                .get_mut(current)
                .ok_or(AdapterError::UnsupportedEvent)? = next_event;
            *out.provenance
                .get_mut(current)
                .ok_or(AdapterError::UnsupportedEvent)? = next_provenance;
            *order
                .get_mut(current)
                .ok_or(AdapterError::UnsupportedEvent)? = current;
            current = next;
        }
    }
    let mut start = 0usize;
    let mut write = 0usize;
    let mut accepted_groups = 0usize;
    let mut previous_receipt = None;
    while start < event_len {
        let receipt = out
            .events
            .get(start)
            .ok_or(AdapterError::UnsupportedEvent)?
            .receipt_ns;
        let mut end = start + 1;
        while end < event_len
            && out
                .events
                .get(end)
                .is_some_and(|event| event.receipt_ns == receipt)
        {
            end += 1;
        }
        let group_len = end - start;
        if group_len > MAX_EVENTS_PER_RECEIPT {
            out.counts.receipt_groups_over_capacity =
                out.counts.receipt_groups_over_capacity.saturating_add(1);
            out.counts.receipt_group_events_refused = out
                .counts
                .receipt_group_events_refused
                .saturating_add(u64::try_from(group_len).map_err(|_| AdapterError::CountOverflow)?);
            account_provenance_range(
                quarantine_queue,
                &mut out.quarantine,
                &out.provenance,
                start..end,
                REASON_RESOURCE_LIMIT,
                clock_origin,
            )?;
        } else if accepted_groups >= MAX_FRAMES {
            out.counts.receipt_groups_over_frame_capacity = out
                .counts
                .receipt_groups_over_frame_capacity
                .saturating_add(1);
            out.counts.receipt_frame_capacity_events_refused = out
                .counts
                .receipt_frame_capacity_events_refused
                .saturating_add(u64::try_from(group_len).map_err(|_| AdapterError::CountOverflow)?);
            account_provenance_range(
                quarantine_queue,
                &mut out.quarantine,
                &out.provenance,
                start..end,
                REASON_RESOURCE_LIMIT,
                clock_origin,
            )?;
        } else if let Some(previous) = previous_receipt {
            let gap = receipt
                .checked_sub(previous)
                .ok_or(AdapterError::CountOverflow)?;
            if gap <= 0 || gap > MAX_FRAME_GAP_NS {
                out.counts.receipt_gap_groups_refused =
                    out.counts.receipt_gap_groups_refused.saturating_add(1);
                out.counts.receipt_gap_events_refused =
                    out.counts.receipt_gap_events_refused.saturating_add(
                        u64::try_from(group_len).map_err(|_| AdapterError::CountOverflow)?,
                    );
                account_provenance_range(
                    quarantine_queue,
                    &mut out.quarantine,
                    &out.provenance,
                    start..end,
                    REASON_INPUT_INVALID,
                    clock_origin,
                )?;
            } else {
                if end > event_len || write > start {
                    return Err(AdapterError::UnsupportedEvent);
                }
                out.events.copy_within(start..end, write);
                out.provenance.copy_within(start..end, write);
                write = write
                    .checked_add(group_len)
                    .ok_or(AdapterError::CountOverflow)?;
                accepted_groups += 1;
                previous_receipt = Some(receipt);
            }
        } else {
            if end > event_len || write > start {
                return Err(AdapterError::UnsupportedEvent);
            }
            out.events.copy_within(start..end, write);
            out.provenance.copy_within(start..end, write);
            write = write
                .checked_add(group_len)
                .ok_or(AdapterError::CountOverflow)?;
            accepted_groups += 1;
            previous_receipt = Some(receipt);
        }
        start = end;
    }
    out.events.truncate(write);
    out.provenance.truncate(write);
    out.counts.events_emitted =
        u64::try_from(out.events.len()).map_err(|_| AdapterError::CountOverflow)?;
    account_final_events(
        quarantine_queue,
        &mut out.quarantine,
        &out.provenance,
        clock_origin,
    )?;
    Ok(())
}

fn fixed_mmps(mps: f32) -> Option<i32> {
    if !mps.is_finite() {
        return None;
    }
    let rounded = oa_ieee::round(f64::from(mps) * 1_000.0);
    if rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return None;
    }
    Some(rounded as i32)
}

// Keep source-specific state and accounting explicit at this narrow adapter boundary.
#[allow(clippy::too_many_arguments)]
fn adapt_sfdps_frame(
    frame: &RecordedFrame,
    issue_receipt_ns: i64,
    max_events: usize,
    next_sequence: &mut u64,
    parser: &mut Parser,
    decoder: &mut Decoder,
    quarantine_queue: &mut QuarantineQueue,
    quarantine: &mut Vec<RecordedQuarantine>,
    conversion_started: Instant,
    frame_started: Instant,
    out: &mut RecordedAdaptation,
) -> Result<(), AdapterError> {
    let already_decoded =
        usize::try_from(out.counts.sfdps_rows_decoded).map_err(|_| AdapterError::WorkBudget)?;
    let remaining_rows = MAX_SFDPS_ROWS_PER_ADAPTATION.saturating_sub(already_decoded);
    if remaining_rows == 0 {
        out.counts.sfdps_frames_over_capacity =
            out.counts.sfdps_frames_over_capacity.saturating_add(1);
        out.counts.unprocessed_frames = out.counts.unprocessed_frames.saturating_add(1);
        quarantine_item(
            quarantine_queue,
            quarantine,
            frame,
            QUARANTINE_FRAME_ROW_LOCATOR,
            REASON_RESOURCE_LIMIT,
            frame.payload.len() as u32,
            oa_feed::MAX_PAYLOAD as u32,
            conversion_started,
            frame_started,
        )?;
        return Ok(());
    }
    let doc = match parser.parse(&frame.payload) {
        Ok(doc) => doc,
        Err(_) => {
            out.counts.sfdps_payload_refused = out.counts.sfdps_payload_refused.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_INPUT_INVALID,
                frame.payload.len() as u32,
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            return Ok(());
        }
    };
    if has_submillisecond_flight_timestamp(&doc, &frame.payload) {
        out.counts.sfdps_timestamp_precision_refused = out
            .counts
            .sfdps_timestamp_precision_refused
            .saturating_add(1);
        out.counts.sfdps_rows_refused = out.counts.sfdps_rows_refused.saturating_add(1);
        quarantine_item(
            quarantine_queue,
            quarantine,
            frame,
            QUARANTINE_FRAME_ROW_LOCATOR,
            REASON_INPUT_INVALID,
            frame.payload.len() as u32,
            oa_feed::MAX_PAYLOAD as u32,
            conversion_started,
            frame_started,
        )?;
        return Ok(());
    }
    let mut events = new_events(remaining_rows.min(4096), 1 << 20);
    let before = decoder.stats();
    let before_rejected = before.rejected;
    let before_field_errors = before.field_errors;
    let (before_unknown_elements, before_unknown_attributes, before_unknown_overflow) = (
        before.unknown_elements,
        before.unknown_attributes,
        before.unknown_overflow,
    );
    let summary = match decoder.decode_prefix(&doc, &frame.payload, &mut events) {
        Ok(summary) => summary,
        Err(_) => {
            out.counts.sfdps_payload_refused = out.counts.sfdps_payload_refused.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                QUARANTINE_FRAME_ROW_LOCATOR,
                REASON_INPUT_INVALID,
                frame.payload.len() as u32,
                oa_feed::MAX_PAYLOAD as u32,
                conversion_started,
                frame_started,
            )?;
            return Ok(());
        }
    };
    out.counts.sfdps_not_decoded = out
        .counts
        .sfdps_not_decoded
        .saturating_add(u64::from(summary.not_decoded));
    out.counts.sfdps_rows_not_decoded_capacity = out
        .counts
        .sfdps_rows_not_decoded_capacity
        .saturating_add(u64::from(summary.not_decoded));
    if summary.not_decoded > 0 {
        quarantine_item(
            quarantine_queue,
            quarantine,
            frame,
            QUARANTINE_FRAME_ROW_LOCATOR,
            REASON_RESOURCE_LIMIT,
            summary.not_decoded,
            u32::try_from(remaining_rows).unwrap_or(u32::MAX),
            conversion_started,
            frame_started,
        )?;
    }
    out.counts.sfdps_sealed_messages = out
        .counts
        .sfdps_sealed_messages
        .saturating_add(u64::from(summary.sealed_window_dropped));
    let stats = decoder.stats();
    out.counts.sfdps_rejected_messages = out.counts.sfdps_rejected_messages.saturating_add(
        stats
            .rejected
            .iter()
            .zip(before_rejected)
            .map(|(a, b)| a.saturating_sub(b))
            .sum::<u64>(),
    );
    out.counts.sfdps_field_errors = out.counts.sfdps_field_errors.saturating_add(
        stats
            .field_errors
            .iter()
            .zip(before_field_errors)
            .map(|(a, b)| a.saturating_sub(b))
            .sum::<u64>(),
    );
    out.counts.sfdps_unknown_elements = out.counts.sfdps_unknown_elements.saturating_add(
        stats
            .unknown_elements
            .saturating_sub(before_unknown_elements),
    );
    out.counts.sfdps_unknown_attributes = out.counts.sfdps_unknown_attributes.saturating_add(
        stats
            .unknown_attributes
            .saturating_sub(before_unknown_attributes),
    );
    out.counts.sfdps_unknown_overflow = out.counts.sfdps_unknown_overflow.saturating_add(
        stats
            .unknown_overflow
            .saturating_sub(before_unknown_overflow),
    );
    out.counts.sfdps_rows_decoded = out
        .counts
        .sfdps_rows_decoded
        .saturating_add(u64::try_from(events.len).map_err(|_| AdapterError::CountOverflow)?);
    let seen_ms = issue_receipt_ns / 1_000_000;
    for row in 0..events.len {
        let has_position = oa_fixm::has_field(&events, row, Field::Position);
        if has_position {
            out.counts.sfdps_positions_unmapped =
                out.counts.sfdps_positions_unmapped.saturating_add(1);
        }
        let mut adapter_stats = SfdpsAdapterStats::default();
        let mut ops = Vec::new();
        ops.try_reserve_exact(MAX_OPS_PER_MESSAGE)
            .map_err(|_| AdapterError::AllocationRefused)?;
        row_ops(
            &events,
            row,
            seen_ms,
            frame.sequence,
            &mut adapter_stats,
            &mut ops,
        );
        out.counts.sfdps_owner_unknown = out
            .counts
            .sfdps_owner_unknown
            .saturating_add(adapter_stats.owner_unknown);
        out.counts.sfdps_handoff_incomplete = out
            .counts
            .sfdps_handoff_incomplete
            .saturating_add(adapter_stats.handoff_incomplete);
        out.counts.sfdps_handoff_unknown_unit = out
            .counts
            .sfdps_handoff_unknown_unit
            .saturating_add(adapter_stats.handoff_unknown_unit);
        out.counts.sfdps_handoff_receiving_only = out
            .counts
            .sfdps_handoff_receiving_only
            .saturating_add(adapter_stats.handoff_receiving_only);
        out.counts.sfdps_departure_not_active = out
            .counts
            .sfdps_departure_not_active
            .saturating_add(adapter_stats.departure_not_active);
        let unusable = adapter_stats
            .no_status
            .saturating_add(adapter_stats.owner_unknown)
            .saturating_add(adapter_stats.handoff_incomplete)
            .saturating_add(adapter_stats.handoff_unknown_unit)
            .saturating_add(adapter_stats.handoff_receiving_only)
            .saturating_add(adapter_stats.departure_not_active);
        out.counts.sfdps_operations_unusable = out
            .counts
            .sfdps_operations_unusable
            .saturating_add(unusable);
        out.counts.sfdps_no_status = out
            .counts
            .sfdps_no_status
            .saturating_add(adapter_stats.no_status);
        if ops.is_empty() {
            out.counts.sfdps_rows_no_operations =
                out.counts.sfdps_rows_no_operations.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                u32::try_from(row)
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or(AdapterError::WorkBudget)?,
                REASON_INPUT_INVALID,
                1,
                MAX_OPS_PER_MESSAGE as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        if out
            .events
            .len()
            .checked_add(ops.len())
            .is_none_or(|wanted| wanted > max_events)
        {
            out.counts.output_capacity_refusals =
                out.counts.output_capacity_refusals.saturating_add(1);
            out.counts.sfdps_rows_refused = out.counts.sfdps_rows_refused.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                u32::try_from(row)
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or(AdapterError::WorkBudget)?,
                REASON_RESOURCE_LIMIT,
                u32::try_from(ops.len()).map_err(|_| AdapterError::WorkBudget)?,
                MAX_OPS_PER_MESSAGE as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(ops.len())
            .map_err(|_| AdapterError::AllocationRefused)?;
        let mut unsupported = false;
        let mut refusal_reasons = 0u16;
        for (operation_index, op) in ops.iter().enumerate() {
            match normalized_flight_operation(NormalizedFlightOperationInput {
                kind: op.kind,
                alt: op.alt,
                fix: op.fix,
                plan_id: op.plan_id,
                state: op.state,
                unit_a: op.unit_a,
                unit_b: op.unit_b,
                address: op.address,
                callsign: op.callsign,
                dep: op.dep,
                arr: op.arr,
            }) {
                Some((operation, address)) => {
                    let Some(source_ns) = op.ts_ms.checked_mul(1_000_000) else {
                        out.counts.invalid_operation_time =
                            out.counts.invalid_operation_time.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_INPUT_INVALID;
                        continue;
                    };
                    if source_ns < 0 {
                        out.counts.invalid_operation_time =
                            out.counts.invalid_operation_time.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_INPUT_INVALID;
                        continue;
                    }
                    if source_ns > frame.receipt_ns {
                        out.counts.source_after_receipt =
                            out.counts.source_after_receipt.saturating_add(1);
                        out.counts.sfdps_source_after_receipt =
                            out.counts.sfdps_source_after_receipt.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_INPUT_INVALID;
                        continue;
                    }
                    if op.key.gufi == 0 {
                        out.counts.invalid_operation_identity =
                            out.counts.invalid_operation_identity.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_IDENTITY_OR_LOCATION_CONFLICT;
                        continue;
                    }
                    if !valid_padded_ascii(&op.centre) {
                        out.counts.invalid_operation_centre =
                            out.counts.invalid_operation_centre.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_IDENTITY_OR_LOCATION_CONFLICT;
                        continue;
                    }
                    if op.kind == OP_ADDRESS && (address == 0 || address > 0x01ff_ffff) {
                        out.counts.invalid_operation_address =
                            out.counts.invalid_operation_address.saturating_add(1);
                        unsupported = true;
                        refusal_reasons |= REASON_IDENTITY_OR_LOCATION_CONFLICT;
                        continue;
                    }
                    pending.push((
                        NormalizedEvent {
                            receipt_ns: issue_receipt_ns,
                            source_ns,
                            sequence: 0,
                            source: SourceKind::Flight,
                            identity: Identity {
                                flight_gufi: op.key.gufi,
                                aircraft_address: address,
                            },
                            centre: op.centre,
                            kind: EventKind::Flight(operation),
                        },
                        operation_index,
                    ));
                }
                None => {
                    if let Some(count) = out
                        .counts
                        .unsupported_operations
                        .get_mut(usize::from(op.kind))
                    {
                        *count = count.saturating_add(1);
                    }
                    unsupported = true;
                    refusal_reasons |= REASON_ALTITUDE_ROUTE_INVALID;
                }
            }
        }
        if unsupported {
            out.counts.sfdps_rows_refused = out.counts.sfdps_rows_refused.saturating_add(1);
            quarantine_item(
                quarantine_queue,
                quarantine,
                frame,
                u32::try_from(row)
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or(AdapterError::WorkBudget)?,
                if refusal_reasons == 0 {
                    REASON_INPUT_INVALID
                } else {
                    refusal_reasons
                },
                u32::try_from(ops.len()).map_err(|_| AdapterError::WorkBudget)?,
                MAX_OPS_PER_MESSAGE as u32,
                conversion_started,
                frame_started,
            )?;
            continue;
        }
        let pending_count =
            u64::try_from(pending.len()).map_err(|_| AdapterError::CountOverflow)?;
        let sequence_end = next_sequence
            .checked_add(pending_count)
            .ok_or(AdapterError::CountOverflow)?;
        out.events
            .try_reserve(pending.len())
            .map_err(|_| AdapterError::AllocationRefused)?;
        out.provenance
            .try_reserve(pending.len())
            .map_err(|_| AdapterError::AllocationRefused)?;
        for (offset, (mut event, operation_index)) in pending.into_iter().enumerate() {
            if event.identity.aircraft_address == 0 {
                out.counts.unbound_flight_updates =
                    out.counts.unbound_flight_updates.saturating_add(1);
            }
            event.sequence = next_sequence
                .checked_add(u64::try_from(offset).map_err(|_| AdapterError::CountOverflow)?)
                .ok_or(AdapterError::CountOverflow)?;
            out.events.push(event);
            out.provenance.push(EventProvenance {
                source: frame.source,
                stream_index: frame.stream_index,
                frame_sequence: frame.sequence,
                input_receipt_ns: frame.receipt_ns,
                jms_timestamp_ms: frame.jms_timestamp_ms,
                mono_ns: frame.mono_ns,
                oaf_flags: frame.oaf_flags,
                frame_byte_offset: frame.byte_offset,
                skipped_before: frame.skipped_before,
                emitted_item_ordinal: u32::try_from(
                    row.saturating_mul(MAX_OPS_PER_MESSAGE)
                        .saturating_add(operation_index),
                )
                .map_err(|_| AdapterError::CountOverflow)?,
                native_flags: events.flags.get(row).copied().unwrap_or(0),
                native_source: events.source.get(row).copied().unwrap_or([0; 2]),
            });
        }
        *next_sequence = sequence_end;
    }
    Ok(())
}

// A7.2: cold normalized operation arguments, mirroring one decoded source operation.
#[derive(Clone, Copy)]
struct NormalizedFlightOperationInput {
    kind: u8,
    alt: Alt,
    fix: [u8; 12],
    plan_id: [u8; 12],
    state: u8,
    unit_a: [u8; 8],
    unit_b: [u8; 8],
    address: u32,
    callsign: [u8; 8],
    dep: [u8; 12],
    arr: [u8; 12],
}

fn normalized_flight_operation(
    input: NormalizedFlightOperationInput,
) -> Option<(crate::input::FlightOperation, u32)> {
    use crate::input::{FlightHandoff, FlightLifecycle, FlightOperation};
    let NormalizedFlightOperationInput {
        kind,
        alt,
        fix,
        plan_id,
        state,
        unit_a,
        unit_b,
        address,
        callsign,
        dep,
        arr,
    } = input;
    let op = match kind {
        OP_SET_PLAN
            if plan_id != [0; 12] && valid_padded_ascii(&plan_id) && !plan_id.contains(&0) =>
        {
            FlightOperation::SetPlan {
                plan_id,
                status: state,
            }
        }
        OP_ASSIGN if alt.form == ALT_SIMPLE => FlightOperation::AssignAltitude {
            altitude_ft: alt.a_ft,
        },
        OP_ASSIGN if alt.form == ALT_NIL => FlightOperation::ClearAssignedAltitude,
        OP_ASSIGN => FlightOperation::AssignedAltitude(native_flight_altitude(alt, fix)?),
        OP_INTERIM if alt.form == ALT_SIMPLE => FlightOperation::SetInterimAltitude {
            altitude_ft: Some(alt.a_ft),
        },
        OP_INTERIM if alt.form == ALT_NIL => {
            FlightOperation::SetInterimAltitude { altitude_ft: None }
        }
        OP_REQUEST if matches!(alt.form, ALT_SIMPLE | ALT_VFR | ALT_VFR_ON_TOP_PLUS) => {
            FlightOperation::RequestedAltitude(native_flight_altitude(alt, [0; 12])?)
        }
        OP_OWNER if valid_padded_ascii(&unit_a) => FlightOperation::SetOwner { unit: unit_a },
        OP_ADDRESS => FlightOperation::SetAddress,
        OP_IDENTITY
            if callsign != [0; 8]
                && dep != [0; 12]
                && arr != [0; 12]
                && valid_padded_ascii(&callsign)
                && valid_padded_ascii(&dep)
                && valid_padded_ascii(&arr) =>
        {
            FlightOperation::SetIdentity {
                callsign,
                departure: dep,
                arrival: arr,
            }
        }
        OP_DEPART => FlightOperation::Lifecycle(FlightLifecycle::Depart),
        OP_COMPLETE => FlightOperation::Lifecycle(FlightLifecycle::Complete),
        OP_CANCEL => FlightOperation::Lifecycle(FlightLifecycle::Cancel),
        OP_DROP => FlightOperation::Lifecycle(FlightLifecycle::Drop),
        OP_HO_INITIATE if valid_padded_ascii(&unit_a) && valid_padded_ascii(&unit_b) => {
            FlightOperation::HandoffEvent(FlightHandoff::Initiate {
                from: unit_a,
                to: unit_b,
            })
        }
        OP_HO_UPDATE if valid_padded_ascii(&unit_b) => {
            FlightOperation::HandoffEvent(FlightHandoff::Update { to: unit_b })
        }
        OP_HO_ACCEPT if valid_padded_ascii(&unit_b) => {
            FlightOperation::HandoffEvent(FlightHandoff::Accept { by: unit_b })
        }
        OP_HO_TAKE if valid_padded_ascii(&unit_b) => {
            FlightOperation::HandoffEvent(FlightHandoff::Take { by: unit_b })
        }
        OP_HO_RETRACT => FlightOperation::HandoffEvent(FlightHandoff::Retract),
        OP_HO_FAIL => FlightOperation::HandoffEvent(FlightHandoff::Fail),
        _ => return None,
    };
    Some((op, if kind == OP_ADDRESS { address } else { 0 }))
}

fn native_flight_altitude(altitude: Alt, fix: [u8; 12]) -> Option<crate::input::FlightAltitude> {
    use crate::input::FlightAltitude as A;
    Some(match altitude.form {
        ALT_SIMPLE => A::Simple(altitude.a_ft),
        ALT_VFR => A::Vfr,
        ALT_VFR_ON_TOP_PLUS => A::VfrOnTopPlus(altitude.a_ft),
        ALT_BLOCK => A::Block {
            above_ft: altitude.a_ft,
            below_ft: altitude.b_ft,
        },
        ALT_AT_FIX if valid_padded_ascii(&fix) && fix.iter().any(|byte| *byte != 0) => A::AtFix {
            before_ft: altitude.a_ft,
            after_ft: altitude.b_ft,
            fix,
        },
        ALT_NIL => A::Nil,
        _ => return None,
    })
}

fn valid_padded_ascii(bytes: &[u8]) -> bool {
    if !bytes.is_ascii() {
        return false;
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    end > 0
        && bytes
            .get(end..)
            .is_some_and(|tail| tail.iter().all(|&b| b == 0))
}

fn padded_ascii_text(bytes: &[u8]) -> Result<&str, AdapterError> {
    if !valid_padded_ascii(bytes) {
        return Err(AdapterError::InvalidCentre);
    }
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let prefix = bytes.get(..end).ok_or(AdapterError::InvalidCentre)?;
    core::str::from_utf8(prefix).map_err(|_| AdapterError::InvalidCentre)
}

/// Converts a bounded prefix of decoded ADS-B positions. Oversize input emits no prefix: the
/// whole recording batch is refused and `over_capacity` records its size. Missing GUFI is emitted
/// as explicit zero for raw positions; ambiguous identity is refused. GUFI is never inferred.
pub fn adapt_positions(
    rows: &[RecordedPosition],
    capacity: usize,
) -> Result<AdapterOutput, AdapterError> {
    if capacity == 0 {
        return Err(AdapterError::ZeroCapacity);
    }
    let mut counts = AdapterCounts {
        examined: u64::try_from(rows.len()).map_err(|_| AdapterError::CountOverflow)?,
        ..AdapterCounts::default()
    };
    let mut events = Vec::with_capacity(rows.len().min(capacity));
    if rows.len() > capacity {
        counts.over_capacity =
            u64::try_from(rows.len() - capacity).map_err(|_| AdapterError::CountOverflow)?;
        return Ok(AdapterOutput { events, counts });
    }
    for row in rows {
        let gufi = match row.identity {
            RecordedIdentity::Unknown => {
                counts.unknown_identity += 1;
                0
            }
            RecordedIdentity::Ambiguous => {
                counts.ambiguous_identity += 1;
                continue;
            }
            RecordedIdentity::Unique { gufi: 0 } => {
                counts.invalid_record += 1;
                continue;
            }
            RecordedIdentity::Unique { gufi } => gufi,
        };
        if row.receipt_ns < 0 || row.source_ns < 0 || row.source_ns > row.receipt_ns {
            counts.invalid_record += 1;
            counts.invalid_time += 1;
            continue;
        }
        if oa_sealed::in_sealed_window(row.receipt_ns) || oa_sealed::in_sealed_window(row.source_ns)
        {
            counts.invalid_record += 1;
            counts.sealed_time += 1;
            continue;
        }
        if row.address == 0 || row.address > 0x01ff_ffff {
            counts.invalid_record += 1;
            counts.invalid_address += 1;
            continue;
        }
        if !(-900_000_000..=900_000_000).contains(&row.latitude_e7)
            || !(-1_800_000_000..=1_800_000_000).contains(&row.longitude_e7)
        {
            counts.invalid_record += 1;
            counts.invalid_coordinate += 1;
            continue;
        }
        if row
            .altitude_ft
            .is_some_and(|v| !(-2_000..=100_000).contains(&v))
        {
            counts.invalid_record += 1;
            counts.invalid_altitude += 1;
            continue;
        }
        if !valid_padded_ascii(&row.centre) {
            counts.invalid_record += 1;
            counts.invalid_centre += 1;
            continue;
        }
        events.push(NormalizedEvent {
            receipt_ns: row.receipt_ns,
            source_ns: row.source_ns,
            sequence: row.sequence,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: gufi,
                aircraft_address: row.address,
            },
            centre: row.centre,
            kind: EventKind::Position {
                latitude_e7: row.latitude_e7,
                longitude_e7: row.longitude_e7,
                altitude_ft: row.altitude_ft,
                east_mmps: row.east_mmps,
                north_mmps: row.north_mmps,
                vertical_rate_fpm: row.vertical_rate_fpm,
            },
        });
        counts.emitted += 1;
    }
    Ok(AdapterOutput { events, counts })
}

/// Serializes supported output rows using the existing normalized protocol's position grammar.
/// The caller must supply complete output; refusal accounting remains with `AdapterOutput`.
pub fn serialize_positions(events: &[NormalizedEvent]) -> Result<String, AdapterError> {
    let mut out = String::from("OA-RS\t1\n");
    for event in events {
        let EventKind::Position {
            latitude_e7,
            longitude_e7,
            altitude_ft,
            east_mmps,
            north_mmps,
            vertical_rate_fpm,
        } = event.kind
        else {
            return Err(AdapterError::UnsupportedEvent);
        };
        if event.source != SourceKind::Surveillance {
            return Err(AdapterError::UnsupportedEvent);
        }
        if !valid_padded_ascii(&event.centre) {
            return Err(AdapterError::InvalidCentre);
        }
        use core::fmt::Write;
        writeln!(
            out,
            "v1\t{}\t{}\t{}\tSURVEILLANCE\t{:032X}\t{:06X}\t{}\tPOS\t{}\t{}\t{}\t{}\t{}\t{}",
            event.receipt_ns,
            event.source_ns,
            event.sequence,
            event.identity.flight_gufi,
            event.identity.aircraft_address,
            padded_ascii_text(&event.centre)?,
            latitude_e7,
            longitude_e7,
            altitude_ft.map_or_else(|| "-".to_owned(), |v| v.to_string()),
            east_mmps,
            north_mmps,
            vertical_rate_fpm.map_or_else(|| "-".to_owned(), |v| v.to_string())
        )
        .map_err(|_| AdapterError::CountOverflow)?;
    }
    Ok(out)
}

/// Serializes the complete subset emitted by the recording adapter as the established OA-RS v1
/// protocol. A row that cannot be represented within 768 bytes refuses the serialization whole.
pub fn serialize_recorded_events(events: &[NormalizedEvent]) -> Result<String, AdapterError> {
    use crate::input::{FlightOperation, MAX_EVENT_LINE_BYTES};
    use core::fmt::Write;
    let version = if events.iter().any(
        |event| matches!(event.kind, EventKind::Flight(op) if flight_operation_requires_v2(op)),
    ) {
        "v2"
    } else {
        "v1"
    };
    let mut out = String::from(if version == "v2" {
        "OA-RS\t2\n"
    } else {
        "OA-RS\t1\n"
    });
    for event in events {
        let (source_name, kind) = match event.source {
            SourceKind::Surveillance => ("SURVEILLANCE", "POS"),
            SourceKind::Flight => ("FLIGHT", "FLIGHT"),
            _ => return Err(AdapterError::UnsupportedEvent),
        };
        if !valid_padded_ascii(&event.centre) || event.identity.aircraft_address > 0x01ff_ffff {
            return Err(AdapterError::InvalidCentre);
        }
        write!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{:032X}\t{:06X}\t{}\t{}",
            version,
            event.receipt_ns,
            event.source_ns,
            event.sequence,
            source_name,
            event.identity.flight_gufi,
            event.identity.aircraft_address,
            padded_ascii_text(&event.centre)?,
            kind
        )
        .map_err(|_| AdapterError::CountOverflow)?;
        match event.kind {
            EventKind::Position {
                latitude_e7,
                longitude_e7,
                altitude_ft,
                east_mmps,
                north_mmps,
                vertical_rate_fpm,
            } => {
                if event.source != SourceKind::Surveillance {
                    return Err(AdapterError::UnsupportedEvent);
                }
                write!(
                    out,
                    "\t{}\t{}\t{}\t{}\t{}\t{}",
                    latitude_e7,
                    longitude_e7,
                    altitude_ft.map_or_else(|| "-".to_owned(), |v| v.to_string()),
                    east_mmps,
                    north_mmps,
                    vertical_rate_fpm.map_or_else(|| "-".to_owned(), |v| v.to_string())
                )
                .map_err(|_| AdapterError::CountOverflow)?;
            }
            EventKind::Flight(op) if event.source == SourceKind::Flight => {
                match op {
                    FlightOperation::SetPlanByKey { .. }
                    | FlightOperation::CancelPlanByKey { .. }
                    | FlightOperation::SetOwnerByKey { .. }
                    | FlightOperation::HandoffByKey { .. } => {
                        // OA-RS carries GUFI rather than the exact NAM key; refuse instead of
                        // silently dropping identity binding while serializing.
                        return Err(AdapterError::UnsupportedEvent);
                    }
                    _ => write_serialized_flight_operation(&mut out, op, version)?,
                }
            }
            _ => return Err(AdapterError::UnsupportedEvent),
        }
        if out
            .len()
            .saturating_sub(out.rfind('\n').map_or(0, |i| i + 1))
            > MAX_EVENT_LINE_BYTES
        {
            return Err(AdapterError::RowTooLong);
        }
        out.push('\n');
    }
    Ok(out)
}

fn write_serialized_flight_operation(
    out: &mut String,
    operation: crate::input::FlightOperation,
    version: &str,
) -> Result<(), AdapterError> {
    use crate::input::{
        FlightHandoff as H, FlightLifecycle as L, FlightOperation as O, HandoffOperation,
    };
    use core::fmt::Write;
    macro_rules! emit {
        ($($argument:tt)*) => {
            write!(out, $($argument)*).map_err(|_| AdapterError::CountOverflow)
        };
    }
    match operation {
        O::SetPlanByKey { .. }
        | O::CancelPlanByKey { .. }
        | O::SetOwnerByKey { .. }
        | O::HandoffByKey { .. } => Err(AdapterError::UnsupportedEvent),
        O::SetPlan { plan_id, status } => {
            emit!("\tPLAN\t{}\t{status}", ascii_field(&plan_id)?)
        }
        O::AssignAltitude { altitude_ft } => emit!("\tASSIGNED\t{altitude_ft}"),
        O::ClearAssignedAltitude => emit!("\tUNASSIGNED"),
        O::SetInterimAltitude { altitude_ft } => emit!(
            "\tINTERIM\t{}",
            altitude_ft.map_or_else(|| "-".to_owned(), |v| v.to_string())
        ),
        O::SetOwner { unit } => emit!("\tOWNER\t{}", ascii_trimmed(&unit)?),
        O::SetAddress => emit!("\tADDRESS"),
        O::SetIdentity {
            callsign,
            departure,
            arrival,
        } => emit!(
            "\tIDENTITY\t{}\t{}\t{}",
            ascii_trimmed(&callsign)?,
            ascii_trimmed(&departure)?,
            ascii_trimmed(&arrival)?
        ),
        O::Handoff { operation, unit } => {
            let name = match operation {
                HandoffOperation::Offer => "HO_OFFER",
                HandoffOperation::Accept => "HO_ACCEPT",
                HandoffOperation::Take => "HO_TAKE",
                HandoffOperation::Fail => "HO_FAIL",
                HandoffOperation::Update if version == "v2" => "HO_UPDATE_LEGACY",
                HandoffOperation::Update => "HO_UPDATE",
                HandoffOperation::Retract => "HO_RETRACT",
            };
            emit!("\t{name}\t{}", ascii_trimmed(&unit)?)
        }
        O::Lifecycle(transition) => {
            let name = match transition {
                L::Depart => "DEPART",
                L::Complete => "COMPLETE",
                L::Cancel => "CANCEL",
                L::Drop => "DROP",
            };
            emit!("\t{name}")
        }
        O::AssignedAltitude(altitude) => write_v2_altitude(out, "ASSIGNED_ALT", altitude),
        O::RequestedAltitude(altitude)
            if matches!(
                altitude,
                crate::input::FlightAltitude::Simple(_)
                    | crate::input::FlightAltitude::Vfr
                    | crate::input::FlightAltitude::VfrOnTopPlus(_)
            ) =>
        {
            write_v2_altitude(out, "REQUESTED_ALT", altitude)
        }
        O::RequestedAltitude(_) => Err(AdapterError::UnsupportedEvent),
        O::HandoffEvent(event) => match event {
            H::Initiate { from, to } => emit!(
                "\tHO_INITIATE\t{}\t{}",
                ascii_trimmed(&from)?,
                ascii_trimmed(&to)?
            ),
            H::Update { to } => emit!("\tHO_UPDATE\t{}", ascii_trimmed(&to)?),
            H::Accept { by } => emit!("\tHO_ACCEPT_NATIVE\t{}", ascii_trimmed(&by)?),
            H::Take { by } => emit!("\tHO_TAKE_NATIVE\t{}", ascii_trimmed(&by)?),
            H::Retract => emit!("\tHO_RETRACT"),
            H::Fail => emit!("\tHO_FAIL_NATIVE"),
        },
    }
}

fn flight_operation_requires_v2(operation: crate::input::FlightOperation) -> bool {
    use crate::input::FlightOperation as O;
    matches!(
        operation,
        O::Lifecycle(_) | O::AssignedAltitude(_) | O::RequestedAltitude(_) | O::HandoffEvent(_)
    )
}

fn write_v2_altitude(
    out: &mut String,
    operation: &str,
    altitude: crate::input::FlightAltitude,
) -> Result<(), AdapterError> {
    use crate::input::FlightAltitude as A;
    use core::fmt::Write;
    let (form, a, b, fix) = match altitude {
        A::Simple(a) => ("SIMPLE", a, 0, None),
        A::Vfr => ("VFR", 0, 0, None),
        A::VfrOnTopPlus(a) => ("VFR_PLUS", a, 0, None),
        A::Block { above_ft, below_ft } => ("BLOCK", above_ft, below_ft, None),
        A::AtFix {
            before_ft,
            after_ft,
            fix,
        } => ("AT_FIX", before_ft, after_ft, Some(fix)),
        A::Nil => ("NIL", 0, 0, None),
    };
    // FIXM fixed-width identifiers are NUL-padded in memory. OA-RS v2 carries
    // their trimmed printable value and the parser restores the fixed width.
    let fix = fix.map_or_else(|| Ok("-".to_owned()), |value| ascii_trimmed(&value))?;
    write!(out, "\t{operation}\t{form}\t{a}\t{b}\t{fix}").map_err(|_| AdapterError::CountOverflow)
}

/// Deterministic companion manifest binding each normalized line to its input recording frame.
pub fn serialize_provenance(
    events: &[NormalizedEvent],
    provenance: &[EventProvenance],
    streams: &[crate::recordings_reader::RecordingStreamProvenance],
) -> Result<String, AdapterError> {
    use core::fmt::Write;
    if events.len() != provenance.len() {
        return Err(AdapterError::UnsupportedEvent);
    }
    let mut out = String::from("OA-RS-PROVENANCE\t1\n");
    for stream in streams {
        let feed = match stream.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        let mut digest = [0u8; 64];
        oa_sha256::to_hex_into(&stream.sha256, &mut digest);
        let digest = core::str::from_utf8(&digest).map_err(|_| AdapterError::InvalidField)?;
        writeln!(
            out,
            "STREAM\t{}\t{}\t{}\t{}\t{}\t{}",
            stream.stream_index,
            feed,
            digest,
            stream.compressed_bytes,
            stream.decoded_bytes,
            u8::from(stream.gzip)
        )
        .map_err(|_| AdapterError::CountOverflow)?;
    }
    for (index, (event, origin)) in events.iter().zip(provenance).enumerate() {
        let source = match event.source {
            SourceKind::Surveillance => "SURVEILLANCE",
            SourceKind::Flight => "FLIGHT",
            SourceKind::Route => "ROUTE",
            SourceKind::Weather => "WEATHER",
            SourceKind::Adaptation => "ADAPTATION",
        };
        let feed = match origin.source {
            RecordingSource::Adsb => "ADSB",
            RecordingSource::Sfdps => "SFDPS",
        };
        writeln!(
            out,
            "EVENT\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:02X}\t{}\t{}\t{:08X}\t{:02X}{:02X}",
            index,
            source,
            event.sequence,
            feed,
            origin.stream_index,
            origin.frame_sequence,
            origin.input_receipt_ns,
            origin.frame_byte_offset,
            origin.jms_timestamp_ms,
            origin
                .mono_ns
                .map_or_else(|| "-".to_owned(), |v| v.to_string()),
            origin.oaf_flags,
            origin.skipped_before,
            origin.emitted_item_ordinal,
            origin.native_flags,
            origin.native_source[0],
            origin.native_source[1]
        )
        .map_err(|_| AdapterError::CountOverflow)?;
    }
    Ok(out)
}

/// Serializes refusal locators and reason metadata. Raw payloads are deliberately not copied;
/// operators use stream index, frame offset, and source row locator to retrieve the original item.
pub fn serialize_quarantine(records: &[RecordedQuarantine]) -> Result<String, AdapterError> {
    use core::fmt::Write;
    if records.len() > MAX_QUARANTINE_RECORDS {
        return Err(AdapterError::WorkBudget);
    }
    let reserve = records
        .len()
        .checked_mul(136)
        .and_then(|size| size.checked_add(64))
        .ok_or(AdapterError::CountOverflow)?;
    let mut out = String::new();
    out.try_reserve_exact(reserve)
        .map_err(|_| AdapterError::AllocationRefused)?;
    out.push_str("OA-REGIONAL-QUARANTINE\t1\n");
    for record in records {
        let source = match record.event.identity.source {
            InputSource::FlightData => "FLIGHT",
            InputSource::Surveillance => "SURVEILLANCE",
            InputSource::Adaptation => "ADAPTATION",
        };
        writeln!(
            out,
            "ITEM\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            source,
            record.stream_index,
            record.frame_sequence,
            record.frame_byte_offset,
            record.row_locator,
            record.event.identity.item_id,
            record.event.reasons.0,
            record.event.clock_domain_id,
            record.event.completed_at_ns,
            u8::from(record.manual_handling_required)
        )
        .map_err(|_| AdapterError::CountOverflow)?;
    }
    Ok(out)
}

fn ascii_field(bytes: &[u8]) -> Result<String, AdapterError> {
    if !bytes.is_ascii() || bytes.contains(&0) {
        return Err(AdapterError::InvalidField);
    }
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| AdapterError::InvalidField)
}

fn ascii_trimmed(bytes: &[u8]) -> Result<String, AdapterError> {
    if !bytes.is_ascii() {
        return Err(AdapterError::InvalidField);
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if bytes
        .get(end..)
        .is_some_and(|tail| tail.iter().any(|&b| b != 0))
    {
        return Err(AdapterError::InvalidField);
    }
    core::str::from_utf8(bytes.get(..end).ok_or(AdapterError::InvalidField)?)
        .map(str::to_owned)
        .map_err(|_| AdapterError::InvalidField)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn row(id: RecordedIdentity, seq: u64) -> RecordedPosition {
        RecordedPosition {
            receipt_ns: 20,
            source_ns: 10,
            sequence: seq,
            address: 0xABCDEF,
            identity: id,
            centre: *b"ZAU1",
            latitude_e7: 410_000_000,
            longitude_e7: -870_000_000,
            altitude_ft: None,
            east_mmps: 123,
            north_mmps: -456,
            vertical_rate_fpm: Some(0),
        }
    }

    #[test]
    fn retains_input_order_and_source_clock_with_explicit_unknown_gufi() {
        let rows = [
            row(RecordedIdentity::Unique { gufi: 0x1122 }, 7),
            row(RecordedIdentity::Unique { gufi: 0x3344 }, 6),
        ];
        let got = adapt_positions(&rows, 2).unwrap();
        assert_eq!(
            got.events
                .iter()
                .map(|e| (e.sequence, e.source_ns, e.identity.flight_gufi))
                .collect::<Vec<_>>(),
            vec![(7, 10, 0x1122), (6, 10, 0x3344)]
        );
        assert_eq!(serialize_positions(&got.events).unwrap(), "OA-RS\t1\nv1\t20\t10\t7\tSURVEILLANCE\t00000000000000000000000000001122\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t-\t123\t-456\t0\nv1\t20\t10\t6\tSURVEILLANCE\t00000000000000000000000000003344\tABCDEF\tZAU1\tPOS\t410000000\t-870000000\t-\t123\t-456\t0\n");
    }

    #[test]
    fn accounts_unknown_ambiguous_invalid_and_oversize_without_guessing() {
        let rows = [
            row(RecordedIdentity::Unknown, 1),
            row(RecordedIdentity::Ambiguous, 2),
            row(RecordedIdentity::Unique { gufi: 0 }, 3),
        ];
        let got = adapt_positions(&rows, 3).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].identity.flight_gufi, 0);
        assert_eq!(got.counts.unknown_identity, 1);
        assert_eq!(got.counts.ambiguous_identity, 1);
        assert_eq!(got.counts.invalid_record, 1);
        let too_many = [
            row(RecordedIdentity::Unique { gufi: 1 }, 1),
            row(RecordedIdentity::Unique { gufi: 2 }, 2),
        ];
        let got = adapt_positions(&too_many, 1).unwrap();
        assert!(got.events.is_empty());
        assert_eq!(got.counts.over_capacity, 1);
    }

    #[test]
    fn empty_and_invalid_time_or_coordinate_have_exact_accounting() {
        let empty = adapt_positions(&[], 4).unwrap();
        assert!(empty.events.is_empty());
        assert_eq!(empty.counts, AdapterCounts::default());
        let mut bad_time = row(RecordedIdentity::Unique { gufi: 5 }, 1);
        bad_time.source_ns = 21;
        let mut bad_coordinate = row(RecordedIdentity::Unique { gufi: 6 }, 2);
        bad_coordinate.latitude_e7 = 900_000_001;
        let got = adapt_positions(&[bad_time, bad_coordinate], 2).unwrap();
        assert!(got.events.is_empty());
        assert_eq!(got.counts.examined, 2);
        assert_eq!(got.counts.invalid_record, 2);
        assert_eq!(
            event_storage_budget(DEVELOPMENT_RECORDING_BUDGET).unwrap(),
            core::mem::size_of::<NormalizedEvent>() * 100_000
        );
    }

    #[test]
    fn recorded_decoder_refusals_are_counted_by_source_and_no_event_is_fabricated() {
        let frames = [
            RecordedFrame {
                source: RecordingSource::Adsb,
                stream_index: 0,
                receipt_ns: 20,
                sequence: 1,
                jms_timestamp_ms: 0,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: 0,
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: b"bad ADS-B payload".to_vec(),
            },
            RecordedFrame {
                source: RecordingSource::Sfdps,
                stream_index: 1,
                receipt_ns: 20,
                sequence: 1,
                jms_timestamp_ms: 0,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: 100,
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: b"<not-xml".to_vec(),
            },
        ];
        let bytes = frames.iter().map(|frame| frame.payload.len() as u64).sum();
        let counts = RecordingCounts {
            total_decoded_bytes: bytes,
            ..RecordingCounts::default()
        };
        let got = adapt_recorded_frames(&frames, counts, 8, *b"ZAU1").unwrap();
        assert!(got.events.is_empty());
        assert!(got.provenance.is_empty());
        assert_eq!(got.counts.adsb_payload_refused, 1);
        assert_eq!(got.counts.sfdps_payload_refused, 1);
        assert_eq!(got.counts.events_emitted, 0);
        assert_eq!(got.quarantine.len(), 2);
        assert_eq!(got.quarantine_stats[0].queued, 1);
        assert_eq!(got.quarantine_stats[1].queued, 1);
        assert_eq!(got.quarantine_stats[2].accepted, 0);
    }

    #[test]
    fn sfdps_three_character_artcc_is_preserved_as_zero_padded_identity() {
        const XML: &str = r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZAU" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><assignedAltitude><simple uom="FEET">12000.0</simple></assignedAltitude></flight></message></ns5:MessageCollection>"#;
        let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.000Z").unwrap();
        let payload = XML.as_bytes().to_vec();
        let frame = RecordedFrame {
            source: RecordingSource::Sfdps,
            stream_index: 0,
            receipt_ns: receipt_ms.checked_mul(1_000_000).unwrap(),
            sequence: 1,
            jms_timestamp_ms: receipt_ms,
            mono_ns: None,
            oaf_flags: 0,
            byte_offset: 0,
            skipped_before: 0,
            kind: oa_feed::Kind::Message,
            payload: payload.clone(),
        };
        let got = adapt_recorded_frames(
            &[frame],
            RecordingCounts {
                total_decoded_bytes: payload.len() as u64,
                ..RecordingCounts::default()
            },
            8,
            *b"ZAU1",
        )
        .unwrap();

        assert_eq!(got.counts.sfdps_rows_decoded, 1);
        assert_eq!(got.counts.invalid_operation_centre, 0);
        assert_eq!(got.counts.sfdps_rows_refused, 0, "{:?}", got.counts);
        assert_eq!(got.events.len(), 1);
        assert!(got.events.iter().all(|event| event.centre == *b"ZAU\0"));
        let serialized = serialize_recorded_events(&got.events).unwrap();
        assert!(serialized.contains("\tZAU\tFLIGHT\tASSIGNED\t12000\n"));
        for line in serialized.lines().skip(1) {
            let round_trip = crate::input::parse_event_line(line).unwrap();
            assert_eq!(round_trip.centre, *b"ZAU\0");
        }
    }

    #[test]
    fn mixed_native_sfdps_row_with_unrepresentable_plan_is_refused_atomically() {
        const XML: &str = r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZAU" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><flightStatus xsi:type="ns5:NasFlightStatusType" fdpsFlightStatus="ACTIVE"/><assignedAltitude><simple uom="FEET">12000</simple></assignedAltitude></flight></message></ns5:MessageCollection>"#;
        let source_ns = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.000Z")
            .unwrap()
            .checked_mul(1_000_000)
            .unwrap();
        let payload = XML.as_bytes().to_vec();
        let frame = RecordedFrame {
            source: RecordingSource::Sfdps,
            stream_index: 0,
            receipt_ns: source_ns,
            sequence: 1,
            jms_timestamp_ms: source_ns / 1_000_000,
            mono_ns: None,
            oaf_flags: 0,
            byte_offset: 0,
            skipped_before: 0,
            kind: oa_feed::Kind::Message,
            payload: payload.clone(),
        };
        let counts = RecordingCounts {
            total_decoded_bytes: payload.len() as u64,
            ..RecordingCounts::default()
        };
        let got = adapt_recorded_frames(&[frame], counts, 16, *b"ZAU1").unwrap();
        assert_eq!(got.counts.sfdps_rows_decoded, 1);
        assert_eq!(got.counts.sfdps_rows_refused, 1);
        assert_eq!(
            got.counts.unsupported_operations[usize::from(OP_SET_PLAN)],
            1
        );
        assert_eq!(got.counts.events_emitted, 0);
        assert!(got.events.is_empty());
        assert_eq!(got.quarantine.len(), 1);
    }

    // REQ: OA-REG-017
    #[test]
    fn fixm_capacity_and_receipt_group_losses_are_separate_and_later_frame_survives() {
        const OPEN: &str = r#"<ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#;
        let repeated_message = r#"<message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:05.000Z"><gufi codeSpace="urn:uuid">11111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message>"#;
        let healthy_message = r#"<message xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX1" source="TH" system="SLC" timestamp="2030-01-02T03:04:06.000Z"><gufi codeSpace="urn:uuid">21111111-2222-3333-4444-555555555555</gufi><controllingUnit unitIdentifier="ZXX1"/></flight></message>"#;
        let mut xml = String::from(OPEN);
        for _ in 0..4_097 {
            xml.push_str(repeated_message);
        }
        xml.push_str("</ns5:MessageCollection>");
        let payload = xml.into_bytes();
        let receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:05.250Z").unwrap();
        let receipt_ns = receipt_ms.checked_mul(1_000_000).unwrap();
        let later_xml = format!("{OPEN}{healthy_message}</ns5:MessageCollection>");
        let later_payload = later_xml.into_bytes();
        let later_receipt_ms = oa_fixm::parse_iso_ms(b"2030-01-02T03:04:06.250Z").unwrap();
        let frames = [
            RecordedFrame {
                source: RecordingSource::Sfdps,
                stream_index: 0,
                receipt_ns,
                sequence: 1,
                jms_timestamp_ms: receipt_ms,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: 0,
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: payload.clone(),
            },
            RecordedFrame {
                source: RecordingSource::Sfdps,
                stream_index: 0,
                receipt_ns: later_receipt_ms.checked_mul(1_000_000).unwrap(),
                sequence: 2,
                jms_timestamp_ms: later_receipt_ms,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: u64::try_from(payload.len()).unwrap(),
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: later_payload.clone(),
            },
        ];
        let got = adapt_recorded_frames(
            &frames,
            RecordingCounts {
                total_decoded_bytes: u64::try_from(payload.len() + later_payload.len()).unwrap(),
                ..RecordingCounts::default()
            },
            5_000,
            *b"ZAU1",
        )
        .unwrap();

        assert_eq!(got.counts.sfdps_rows_decoded, 4_097);
        assert_eq!(got.counts.sfdps_rows_not_decoded_capacity, 1);
        assert_eq!(got.counts.receipt_groups_over_capacity, 1);
        assert_eq!(got.counts.receipt_group_events_refused, 4_096);
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.counts.events_emitted, 1);
        assert_eq!(got.quarantine.len(), 4_097);
        let capacity_loss = got
            .quarantine
            .iter()
            .find(|record| {
                record.frame_sequence == 1 && record.row_locator == QUARANTINE_FRAME_ROW_LOCATOR
            })
            .expect("capacity loss has a frame-level locator");
        assert_eq!(capacity_loss.event.reasons.0, REASON_RESOURCE_LIMIT);
        assert_eq!(got.quarantine_stats[0].queued, 128);
        assert_eq!(got.quarantine_stats[0].queue_full_manual_events, 3_969);
    }

    #[test]
    fn full_source_queue_keeps_every_refusal_visible_for_manual_handling() {
        let frames = (0..=oa_quarantine::MAX_PENDING_PER_SOURCE)
            .map(|index| RecordedFrame {
                source: RecordingSource::Adsb,
                stream_index: 0,
                receipt_ns: 1_000_000_000,
                sequence: u64::try_from(index).unwrap(),
                jms_timestamp_ms: 1_000,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: u64::try_from(index * 32).unwrap(),
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: b"bad ADS-B payload".to_vec(),
            })
            .collect::<Vec<_>>();
        let decoded_bytes = frames.iter().map(|frame| frame.payload.len() as u64).sum();
        let counts = RecordingCounts {
            total_decoded_bytes: decoded_bytes,
            ..RecordingCounts::default()
        };
        let got = adapt_recorded_frames(&frames, counts, 8, *b"ZAU1").unwrap();
        assert_eq!(got.counts.adsb_payload_refused, 129);
        assert_eq!(got.quarantine.len(), 129);
        assert_eq!(got.quarantine_stats[1].queued, 128);
        assert_eq!(got.quarantine_stats[1].queue_full_manual_events, 1);
        assert!(got
            .quarantine
            .iter()
            .take(128)
            .all(|item| !item.manual_handling_required));
        assert!(got.quarantine.last().unwrap().manual_handling_required);
    }

    #[test]
    fn native_sfdps_operation_codes_map_without_field_or_transition_loss() {
        use crate::input::{
            FlightAltitude as A, FlightHandoff as H, FlightLifecycle as L, FlightOperation as O,
        };

        fn input(kind: u8, alt: Alt) -> NormalizedFlightOperationInput {
            NormalizedFlightOperationInput {
                kind,
                alt,
                fix: [0; 12],
                plan_id: *b"PLAN00000001",
                state: oa_flight::ST_ACTIVE,
                unit_a: *b"FROM    ",
                unit_b: *b"TO      ",
                address: 0x00ab_cdef,
                callsign: *b"CALL123 ",
                dep: *b"KATL        ",
                arr: *b"KORD        ",
            }
        }

        let simple = Alt {
            form: ALT_SIMPLE,
            a_ft: 12_000,
            b_ft: 0,
        };
        assert_eq!(
            normalized_flight_operation(input(OP_SET_PLAN, Alt::default()))
                .unwrap()
                .0,
            O::SetPlan {
                plan_id: *b"PLAN00000001",
                status: oa_flight::ST_ACTIVE
            }
        );
        for (kind, expected) in [
            (OP_DEPART, L::Depart),
            (OP_COMPLETE, L::Complete),
            (OP_CANCEL, L::Cancel),
            (OP_DROP, L::Drop),
        ] {
            assert_eq!(
                normalized_flight_operation(input(kind, Alt::default()))
                    .unwrap()
                    .0,
                O::Lifecycle(expected)
            );
        }
        assert_eq!(
            normalized_flight_operation(input(OP_ASSIGN, simple))
                .unwrap()
                .0,
            O::AssignAltitude {
                altitude_ft: 12_000
            }
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_ASSIGN,
                Alt {
                    form: ALT_BLOCK,
                    a_ft: 10_000,
                    b_ft: 14_000
                }
            ))
            .unwrap()
            .0,
            O::AssignedAltitude(A::Block {
                above_ft: 10_000,
                below_ft: 14_000
            })
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_ASSIGN,
                Alt {
                    form: ALT_VFR,
                    a_ft: 0,
                    b_ft: 0
                }
            ))
            .unwrap()
            .0,
            O::AssignedAltitude(A::Vfr)
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_ASSIGN,
                Alt {
                    form: ALT_VFR_ON_TOP_PLUS,
                    a_ft: 5_500,
                    b_ft: 0
                }
            ))
            .unwrap()
            .0,
            O::AssignedAltitude(A::VfrOnTopPlus(5_500))
        );
        let mut at_fix = input(
            OP_ASSIGN,
            Alt {
                form: ALT_AT_FIX,
                a_ft: 8_000,
                b_ft: 7_000,
            },
        );
        at_fix.fix = *b"FIX         ";
        assert_eq!(
            normalized_flight_operation(at_fix).unwrap().0,
            O::AssignedAltitude(A::AtFix {
                before_ft: 8_000,
                after_ft: 7_000,
                fix: *b"FIX         "
            })
        );
        assert_eq!(
            normalized_flight_operation(input(OP_INTERIM, simple))
                .unwrap()
                .0,
            O::SetInterimAltitude {
                altitude_ft: Some(12_000)
            }
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_REQUEST,
                Alt {
                    form: ALT_VFR_ON_TOP_PLUS,
                    a_ft: 5_500,
                    b_ft: 0
                }
            ))
            .unwrap()
            .0,
            O::RequestedAltitude(A::VfrOnTopPlus(5_500))
        );
        assert_eq!(
            normalized_flight_operation(input(OP_OWNER, Alt::default()))
                .unwrap()
                .0,
            O::SetOwner { unit: *b"FROM    " }
        );
        assert_eq!(
            normalized_flight_operation(input(OP_ADDRESS, Alt::default())),
            Some((O::SetAddress, 0x00ab_cdef))
        );
        assert_eq!(
            normalized_flight_operation(input(OP_IDENTITY, Alt::default()))
                .unwrap()
                .0,
            O::SetIdentity {
                callsign: *b"CALL123 ",
                departure: *b"KATL        ",
                arrival: *b"KORD        "
            }
        );
        for (kind, expected) in [
            (
                OP_HO_INITIATE,
                H::Initiate {
                    from: *b"FROM    ",
                    to: *b"TO      ",
                },
            ),
            (OP_HO_UPDATE, H::Update { to: *b"TO      " }),
            (OP_HO_ACCEPT, H::Accept { by: *b"TO      " }),
            (OP_HO_TAKE, H::Take { by: *b"TO      " }),
            (OP_HO_RETRACT, H::Retract),
            (OP_HO_FAIL, H::Fail),
        ] {
            assert_eq!(
                normalized_flight_operation(input(kind, Alt::default()))
                    .unwrap()
                    .0,
                O::HandoffEvent(expected)
            );
        }
        assert_eq!(
            normalized_flight_operation(input(
                OP_ASSIGN,
                Alt {
                    form: ALT_AT_FIX,
                    a_ft: 8_000,
                    b_ft: 7_000
                }
            )),
            None,
            "an at-fix altitude without its source fix is not normalized"
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_INTERIM,
                Alt {
                    form: ALT_BLOCK,
                    a_ft: 8_000,
                    b_ft: 7_000
                }
            )),
            None,
            "the native core does not admit block interim values"
        );
        assert_eq!(
            normalized_flight_operation(input(
                OP_REQUEST,
                Alt {
                    form: ALT_BLOCK,
                    a_ft: 8_000,
                    b_ft: 7_000
                }
            )),
            None,
            "the native core does not admit block requested values"
        );
    }

    #[test]
    fn serializes_flight_events_and_exact_frame_provenance() {
        use crate::input::FlightOperation;
        let event = NormalizedEvent {
            receipt_ns: 20,
            source_ns: 10,
            sequence: 3,
            source: SourceKind::Flight,
            identity: Identity {
                flight_gufi: 0x123,
                aircraft_address: 0,
            },
            centre: *b"ZAU1",
            kind: EventKind::Flight(FlightOperation::SetOwner {
                unit: *b"SECTOR1\0",
            }),
        };
        let expected = "OA-RS\t1\nv1\t20\t10\t3\tFLIGHT\t00000000000000000000000000000123\t000000\tZAU1\tFLIGHT\tOWNER\tSECTOR1\n";
        assert_eq!(serialize_recorded_events(&[event]).unwrap(), expected);
        let origin = EventProvenance {
            source: RecordingSource::Sfdps,
            stream_index: 1,
            frame_sequence: 42,
            input_receipt_ns: 1_234_567_890,
            jms_timestamp_ms: 9,
            mono_ns: Some(88),
            oaf_flags: 2,
            frame_byte_offset: 1234,
            skipped_before: 5,
            emitted_item_ordinal: 7,
            native_flags: 0x12,
            native_source: *b"TR",
        };
        let stream = crate::recordings_reader::RecordingStreamProvenance {
            source: RecordingSource::Sfdps,
            stream_index: 1,
            sha256: [0xAB; 32],
            compressed_bytes: 80,
            decoded_bytes: 80,
            gzip: false,
        };
        let manifest = serialize_provenance(&[event], &[origin], &[stream]).unwrap();
        assert_eq!(manifest, format!("OA-RS-PROVENANCE\t1\nSTREAM\t1\tSFDPS\t{}\t80\t80\t0\nEVENT\t0\tFLIGHT\t3\tSFDPS\t1\t42\t1234567890\t1234\t9\t88\t02\t5\t7\t00000012\t5452\n", "ab".repeat(32)));
    }

    #[test]
    fn native_flight_operations_use_explicit_oa_rs_v2_and_round_trip() {
        use crate::input::{
            FlightAltitude, FlightHandoff, FlightLifecycle, FlightOperation, MAX_EVENT_LINE_BYTES,
        };
        let operations = [
            FlightOperation::Lifecycle(FlightLifecycle::Depart),
            FlightOperation::AssignedAltitude(FlightAltitude::AtFix {
                before_ft: 8_000,
                after_ft: 7_000,
                fix: *b"FIX         ",
            }),
            FlightOperation::RequestedAltitude(FlightAltitude::VfrOnTopPlus(5_500)),
            FlightOperation::HandoffEvent(FlightHandoff::Initiate {
                from: *b"FROM\0\0\0\0",
                to: *b"TO\0\0\0\0\0\0",
            }),
            FlightOperation::HandoffEvent(FlightHandoff::Update {
                to: *b"NEXT\0\0\0\0",
            }),
            FlightOperation::HandoffEvent(FlightHandoff::Retract),
        ];
        let events: Vec<_> = operations
            .into_iter()
            .enumerate()
            .map(|(index, operation)| NormalizedEvent {
                receipt_ns: 2_000_000_000 + index as i64 * 1_000_000,
                source_ns: 1_000_000_000 + index as i64 * 1_000_000,
                sequence: index as u64 + 1,
                source: SourceKind::Flight,
                identity: Identity {
                    flight_gufi: 0x1234,
                    aircraft_address: 0,
                },
                centre: *b"ZAU1",
                kind: EventKind::Flight(operation),
            })
            .collect();
        let serialized = serialize_recorded_events(&events).unwrap();
        assert!(serialized.starts_with("OA-RS\t2\n"));
        for (line, expected) in serialized.lines().skip(1).zip(&events) {
            assert!(line.starts_with("v2\t"));
            assert!(line.len() <= MAX_EVENT_LINE_BYTES);
            assert_eq!(crate::input::parse_event_line(line).unwrap(), *expected);
        }
        let old_version_line = serialized
            .lines()
            .nth(1)
            .unwrap()
            .replacen("v2\t", "v1\t", 1);
        assert_eq!(
            crate::input::parse_event_line(&old_version_line),
            Err(crate::input::InputError::InvalidOperation),
            "OA-RS v1 must not silently accept the v2 lifecycle grammar"
        );
        assert_eq!(serialized.lines().count(), events.len() + 1);
    }

    #[test]
    fn ground_status_is_never_promoted_to_airborne() {
        assert_eq!(adsb_ground_class(F_GROUND, 0), AdsGroundClass::Ground);
        assert_eq!(adsb_ground_class(0, NONE_I32), AdsGroundClass::Unknown);
        assert_eq!(adsb_ground_class(0, 12_000), AdsGroundClass::Airborne);
    }

    fn event(source: SourceKind, sequence: u64) -> NormalizedEvent {
        let kind = match source {
            SourceKind::Surveillance => EventKind::Position {
                latitude_e7: 1,
                longitude_e7: 2,
                altitude_ft: Some(10_000),
                east_mmps: 3,
                north_mmps: 4,
                vertical_rate_fpm: None,
            },
            SourceKind::Flight => EventKind::Flight(crate::input::FlightOperation::SetOwner {
                unit: *b"SECTOR1\0",
            }),
            _ => unreachable!(),
        };
        NormalizedEvent {
            receipt_ns: 2_000_000_000,
            source_ns: 1_000_000_000,
            sequence,
            source,
            identity: Identity {
                flight_gufi: 1,
                aircraft_address: 0x123456,
            },
            centre: *b"ZAU1",
            kind,
        }
    }

    fn provenance(source: RecordingSource, receipt_ns: i64, ordinal: u32) -> EventProvenance {
        EventProvenance {
            source,
            stream_index: 0,
            frame_sequence: ordinal as u64,
            input_receipt_ns: receipt_ns,
            jms_timestamp_ms: 0,
            mono_ns: None,
            oaf_flags: 0,
            frame_byte_offset: u64::from(ordinal) * 100,
            skipped_before: 0,
            emitted_item_ordinal: ordinal,
            native_flags: 0,
            native_source: [0; 2],
        }
    }

    #[test]
    fn receipt_cutoff_ceil_preserves_boundaries_and_refuses_negative_sealed_and_overflow() {
        assert_eq!(issue_cutoff(1_000_000_000), Ok(1_000_000_000));
        assert_eq!(issue_cutoff(1_000_000_001), Ok(2_000_000_000));
        assert_eq!(issue_cutoff(-1), Err(CutoffRefusal::Negative));
        assert_eq!(issue_cutoff(0), Err(CutoffRefusal::Zero));
        assert_eq!(issue_cutoff(i64::MAX), Err(CutoffRefusal::Overflow));
        let window = oa_sealed::Sealed::production()
            .windows()
            .first()
            .expect("production sealed window");
        assert_eq!(
            issue_cutoff(window.from_ns),
            Err(CutoffRefusal::SealedReceipt)
        );
        assert_eq!(
            issue_cutoff(window.from_ns - 1),
            Err(CutoffRefusal::SealedCutoff)
        );
    }

    #[test]
    fn adapter_counts_each_cutoff_refusal_reason_from_headers() {
        let sealed = oa_sealed::Sealed::production()
            .windows()
            .first()
            .expect("production sealed window");
        let receipts = [-1, 0, sealed.from_ns - 1, sealed.from_ns, i64::MAX];
        let frames = receipts
            .iter()
            .enumerate()
            .map(|(index, receipt_ns)| RecordedFrame {
                source: RecordingSource::Adsb,
                stream_index: 0,
                receipt_ns: *receipt_ns,
                sequence: index as u64,
                jms_timestamp_ms: 0,
                mono_ns: None,
                oaf_flags: 0,
                byte_offset: index as u64 * 40,
                skipped_before: 0,
                kind: oa_feed::Kind::Message,
                payload: b"bad".to_vec(),
            })
            .collect::<Vec<_>>();
        let counts = RecordingCounts {
            total_decoded_bytes: 15,
            ..RecordingCounts::default()
        };
        let got = adapt_recorded_frames(&frames, counts, 8, *b"ZAU1").unwrap();
        assert_eq!(got.counts.cutoff_negative_refused, 1);
        assert_eq!(got.counts.cutoff_zero_refused, 1);
        assert_eq!(got.counts.cutoff_sealed_issue_refused, 1);
        assert_eq!(got.counts.cutoff_sealed_receipt_refused, 1);
        assert_eq!(got.counts.cutoff_overflow_refused, 1);
        assert_eq!(got.counts.adsb_payload_refused, 0);
    }

    #[test]
    fn coalesced_cutoff_sorts_category_then_original_receipt_with_provenance_attached() {
        let mut output = RecordedAdaptation {
            events: vec![
                event(SourceKind::Flight, 2),
                event(SourceKind::Flight, 1),
                event(SourceKind::Surveillance, 0),
            ],
            provenance: vec![
                provenance(RecordingSource::Sfdps, 1_900_000_000, 2),
                provenance(RecordingSource::Sfdps, 1_100_000_000, 1),
                provenance(RecordingSource::Adsb, 1_999_999_999, 0),
            ],
            counts: zero_recorded_adapter_counts(),
            recording: RecordingCounts::default(),
            quarantine: Vec::new(),
            quarantine_stats: zero_quarantine_stats(),
        };
        sort_and_refuse_oversize_receipt_groups(&mut output).unwrap();
        assert_eq!(
            output
                .events
                .iter()
                .map(|e| (e.receipt_ns, e.source, e.sequence))
                .collect::<Vec<_>>(),
            vec![
                (2_000_000_000, SourceKind::Surveillance, 0),
                (2_000_000_000, SourceKind::Flight, 1),
                (2_000_000_000, SourceKind::Flight, 2)
            ]
        );
        assert_eq!(
            output
                .provenance
                .iter()
                .map(|p| p.input_receipt_ns)
                .collect::<Vec<_>>(),
            vec![1_999_999_999, 1_100_000_000, 1_900_000_000]
        );
    }

    #[test]
    fn complete_overfull_issue_group_is_refused_and_counted() {
        let mut output = RecordedAdaptation {
            events: Vec::new(),
            provenance: Vec::new(),
            counts: zero_recorded_adapter_counts(),
            recording: RecordingCounts::default(),
            quarantine: Vec::new(),
            quarantine_stats: zero_quarantine_stats(),
        };
        for index in 0..=crate::session::MAX_EVENTS_PER_RECEIPT {
            output
                .events
                .push(event(SourceKind::Surveillance, index as u64));
            output.provenance.push(provenance(
                RecordingSource::Adsb,
                1_000_000_001,
                index as u32,
            ));
        }
        sort_and_refuse_oversize_receipt_groups(&mut output).unwrap();
        assert!(output.events.is_empty());
        assert_eq!(output.counts.receipt_groups_over_capacity, 1);
        assert_eq!(output.counts.receipt_group_events_refused, 257);
        assert_eq!(output.counts.events_emitted, 0);
    }

    #[test]
    fn runtime_gap_over_sixty_seconds_refuses_the_later_complete_group() {
        let mut first = event(SourceKind::Surveillance, 1);
        first.receipt_ns = 1_000_000_000;
        let mut later = event(SourceKind::Surveillance, 2);
        later.receipt_ns = 62_000_000_000;
        let mut output = RecordedAdaptation {
            events: vec![later, first],
            provenance: vec![
                provenance(RecordingSource::Adsb, 62_000_000_000, 2),
                provenance(RecordingSource::Adsb, 1_000_000_000, 1),
            ],
            counts: zero_recorded_adapter_counts(),
            recording: RecordingCounts::default(),
            quarantine: Vec::new(),
            quarantine_stats: zero_quarantine_stats(),
        };
        sort_and_refuse_oversize_receipt_groups(&mut output).unwrap();
        assert_eq!(
            output
                .events
                .iter()
                .map(|e| (e.receipt_ns, e.sequence))
                .collect::<Vec<_>>(),
            vec![(1_000_000_000, 1)]
        );
        assert_eq!(output.counts.receipt_gap_groups_refused, 1);
        assert_eq!(output.counts.receipt_gap_events_refused, 1);
    }

    #[test]
    fn fixm_timestamp_guard_refuses_only_nonzero_submillisecond_precision() {
        let mut parser = Parser::new(XmlLimits::default()).unwrap();
        let finer = b"<flight timestamp=\"2020-01-01T00:00:00.1234Z\"/>";
        let doc = parser.parse(finer).unwrap();
        assert!(has_submillisecond_flight_timestamp(&doc, finer));
        let exact = b"<flight timestamp=\"2020-01-01T00:00:00.123000Z\"/>";
        let doc = parser.parse(exact).unwrap();
        assert!(!has_submillisecond_flight_timestamp(&doc, exact));
        let entity = b"<flight timestamp=\"2020-01-01T00:00:00.123&#52;Z\"/>";
        let doc = parser.parse(entity).unwrap();
        assert!(has_submillisecond_flight_timestamp(&doc, entity));
    }
}

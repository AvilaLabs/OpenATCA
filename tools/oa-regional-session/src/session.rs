//! Bounded receipt-time event staging for the regional session. Shell (B1, B3, B7).
//!
//! One receipt-time group is held until a later timestamp or EOF establishes its boundary. The
//! later event is not applied until the previous group is returned, so every consumer sees only
//! the admitted prefix. Older source timestamps are retained for downstream stale-data rules.

use crate::input::{NormalizedEvent, SourceKind};

/// Maximum events sharing one receipt timestamp. Reaching the bound refuses the next row and
/// reports it; no event is silently replaced or dropped (RULESET C2, C7, E6).
pub const MAX_EVENTS_PER_RECEIPT: usize = 256;
/// Hour-profile-only ceiling for a dense normalized receipt cutoff.
pub const RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT: usize = 8_192;

/// Maximum normalized input events accepted by one invocation.
pub const MAX_SESSION_EVENTS: usize = 100_000;
/// Versioned offline full-day cursor bound. Callers must still account for every source row and
/// stop on this refusal; this is not a parser or archive-size allowance.
pub const MAX_RECORDED_PROFILE_EVENTS: usize = 10_000_000;

/// Staged, same-receipt event group. A7.2: cold shell struct; not subsystem state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptBatch {
    pub receipt_ns: i64,
    /// Older source stamps are handed downstream; this count does not refuse those events.
    pub source_clock_regressions: u64,
    pub events: Vec<NormalizedEvent>,
}

/// Input accounting only. A7.2: cold shell struct; does not claim subsystem output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEvidence {
    pub receipt_ns: i64,
    pub input_events: u16,
    pub source_events: [u16; 5],
    pub source_clock_regressions: u64,
}

/// Errors found while admitting a normalized prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    CursorClosed,
    InvalidTime,
    SealedTimestamp,
    ReceiptRegression,
    EqualTimeOrderRegression,
    SequenceRegression,
    ReceiptGroupFull,
    EventCountOverflow,
    SessionFull,
    NoPendingGroup,
}

/// Bounded cursor over one strictly receipt-ordered normalized timeline. A7.2: cold shell struct.
pub struct SessionCursor {
    pending: Vec<NormalizedEvent>,
    receipt_ns: Option<i64>,
    prior_receipt_ns: Option<i64>,
    prior_source_ns: [Option<i64>; 5],
    prior_sequence: [Option<u64>; 5],
    source_clock_regressions: u64,
    total_events: u64,
    max_events: usize,
    max_events_per_receipt: usize,
    closed: bool,
}

/// Constructs the bounded shell cursor; allocation occurs at initialization (RULESET C6).
pub fn session_cursor_new() -> SessionCursor {
    SessionCursor {
        pending: Vec::with_capacity(MAX_EVENTS_PER_RECEIPT),
        receipt_ns: None,
        prior_receipt_ns: None,
        prior_source_ns: [None; 5],
        prior_sequence: [None; 5],
        source_clock_regressions: 0,
        total_events: 0,
        max_events: MAX_SESSION_EVENTS,
        max_events_per_receipt: MAX_EVENTS_PER_RECEIPT,
        closed: false,
    }
}

/// Constructs a bounded streaming cursor for an explicitly selected offline profile. This legacy
/// constructor retains the default per-receipt cap.
pub fn session_cursor_new_with_limit(max_events: usize) -> Result<SessionCursor, SessionError> {
    session_cursor_new_with_profile_limits(max_events, MAX_EVENTS_PER_RECEIPT)
}

/// Constructs a bounded cursor with explicit total-event and receipt-group budgets. The larger
/// receipt-group limit is reserved for the recorded-hour profile; ordinary profiles keep 256.
pub fn session_cursor_new_with_profile_limits(
    max_events: usize,
    max_events_per_receipt: usize,
) -> Result<SessionCursor, SessionError> {
    if max_events == 0 || max_events > MAX_RECORDED_PROFILE_EVENTS {
        return Err(SessionError::SessionFull);
    }
    if !matches!(
        max_events_per_receipt,
        MAX_EVENTS_PER_RECEIPT | RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
    ) {
        return Err(SessionError::ReceiptGroupFull);
    }
    Ok(SessionCursor {
        pending: Vec::with_capacity(max_events_per_receipt),
        receipt_ns: None,
        prior_receipt_ns: None,
        prior_source_ns: [None; 5],
        prior_sequence: [None; 5],
        source_clock_regressions: 0,
        total_events: 0,
        max_events,
        max_events_per_receipt,
        closed: false,
    })
}

/// Returns the number of events admitted to this cursor.
pub fn session_total_events(cursor: &SessionCursor) -> u64 {
    cursor.total_events
}

fn source_index(source: SourceKind) -> usize {
    match source {
        SourceKind::Surveillance => 0,
        SourceKind::Flight => 1,
        SourceKind::Route => 2,
        SourceKind::Weather => 3,
        SourceKind::Adaptation => 4,
    }
}

fn compact_profile_batch(
    events: Vec<NormalizedEvent>,
    max_events_per_receipt: usize,
) -> Vec<NormalizedEvent> {
    if max_events_per_receipt == RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT {
        events.into_boxed_slice().into_vec()
    } else {
        events
    }
}

/// Admits one normalized event and closes the prior receipt group when the timestamp advances.
/// The incoming event remains pending and is not part of the returned batch.
pub fn push_event(
    cursor: &mut SessionCursor,
    event: NormalizedEvent,
) -> Result<Option<ReceiptBatch>, SessionError> {
    if cursor.closed {
        return Err(SessionError::CursorClosed);
    }
    if event.receipt_ns < 0
        || event.input_receipt_ns < 0
        || event.source_ns < 0
        || event.input_receipt_ns > event.receipt_ns
        || event.source_ns > event.input_receipt_ns
    {
        return Err(SessionError::InvalidTime);
    }
    if oa_sealed::in_sealed_window(event.source_ns)
        || oa_sealed::in_sealed_window(event.receipt_ns)
        || oa_sealed::in_sealed_window(event.input_receipt_ns)
    {
        return Err(SessionError::SealedTimestamp);
    }
    if cursor
        .prior_receipt_ns
        .is_some_and(|prior| event.receipt_ns < prior)
    {
        return Err(SessionError::ReceiptRegression);
    }
    if cursor.total_events >= cursor.max_events as u64 {
        return Err(SessionError::SessionFull);
    }

    let source_ix = source_index(event.source);
    let previous_source_ns = cursor.prior_source_ns.get(source_ix).copied().flatten();
    let previous_sequence = cursor.prior_sequence.get(source_ix).copied().flatten();
    let source_regressed = previous_source_ns.is_some_and(|prior| event.source_ns < prior);
    if previous_sequence.is_some_and(|prior| event.sequence <= prior) {
        return Err(SessionError::SequenceRegression);
    }
    if cursor.receipt_ns == Some(event.receipt_ns) {
        if cursor
            .pending
            .last()
            .is_some_and(|prior| event.source < prior.source)
        {
            return Err(SessionError::EqualTimeOrderRegression);
        }
        if cursor.pending.len() >= cursor.max_events_per_receipt {
            return Err(SessionError::ReceiptGroupFull);
        }
    }
    let Some(prior_source_slot) = cursor.prior_source_ns.get_mut(source_ix) else {
        return Err(SessionError::EventCountOverflow);
    };
    let Some(prior_sequence_slot) = cursor.prior_sequence.get_mut(source_ix) else {
        return Err(SessionError::EventCountOverflow);
    };

    let next_total = cursor
        .total_events
        .checked_add(1)
        .ok_or(SessionError::EventCountOverflow)?;
    let same_receipt = cursor.receipt_ns == Some(event.receipt_ns);
    let regression_base = if same_receipt {
        cursor.source_clock_regressions
    } else {
        0
    };
    let next_regressions = if source_regressed {
        Some(
            regression_base
                .checked_add(1)
                .ok_or(SessionError::EventCountOverflow)?,
        )
    } else {
        Some(regression_base)
    };

    let completed = match cursor.receipt_ns {
        Some(current) if event.receipt_ns > current => {
            let events = core::mem::replace(
                &mut cursor.pending,
                Vec::with_capacity(cursor.max_events_per_receipt),
            );
            let events = compact_profile_batch(events, cursor.max_events_per_receipt);
            let source_clock_regressions = cursor.source_clock_regressions;
            cursor.source_clock_regressions = 0;
            Some(ReceiptBatch {
                receipt_ns: current,
                source_clock_regressions,
                events,
            })
        }
        Some(current) if event.receipt_ns < current => return Err(SessionError::ReceiptRegression),
        _ => None,
    };

    cursor.pending.push(event);
    cursor.receipt_ns = Some(event.receipt_ns);
    cursor.prior_receipt_ns = Some(event.receipt_ns);
    *prior_source_slot = Some(event.source_ns);
    *prior_sequence_slot = Some(event.sequence);
    cursor.source_clock_regressions = next_regressions.ok_or(SessionError::EventCountOverflow)?;
    cursor.total_events = next_total;
    Ok(completed)
}

/// Closes the final group at EOF. Repeated calls after EOF return `Ok(None)`.
pub fn flush_receipt_group(
    cursor: &mut SessionCursor,
) -> Result<Option<ReceiptBatch>, SessionError> {
    if cursor.pending.is_empty() {
        cursor.closed = true;
        return Ok(None);
    }
    let Some(receipt_ns) = cursor.receipt_ns else {
        return Err(SessionError::NoPendingGroup);
    };
    let events = core::mem::replace(
        &mut cursor.pending,
        Vec::with_capacity(cursor.max_events_per_receipt),
    );
    let events = compact_profile_batch(events, cursor.max_events_per_receipt);
    cursor.receipt_ns = None;
    let source_clock_regressions = cursor.source_clock_regressions;
    cursor.source_clock_regressions = 0;
    cursor.closed = true;
    Ok(Some(ReceiptBatch {
        receipt_ns,
        source_clock_regressions,
        events,
    }))
}

/// Summarizes a batch as input accounting only; it does not claim tracking, flight, alert or
/// display output. This public input-only helper does not execute the subsystem bridge.
pub fn summarize_input_batch(batch: &ReceiptBatch) -> Result<FrameEvidence, SessionError> {
    summarize_input_batch_with_limit(batch, MAX_EVENTS_PER_RECEIPT)
}

/// Summarizes input using a profile’s explicit receipt-group bound.
pub fn summarize_input_batch_with_limit(
    batch: &ReceiptBatch,
    max_events_per_receipt: usize,
) -> Result<FrameEvidence, SessionError> {
    if !matches!(
        max_events_per_receipt,
        MAX_EVENTS_PER_RECEIPT | RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
    ) || batch.events.len() > max_events_per_receipt
    {
        return Err(SessionError::ReceiptGroupFull);
    }
    let mut counts = [0u16; 5];
    for event in &batch.events {
        let ix = source_index(event.source);
        let Some(slot) = counts.get_mut(ix) else {
            return Err(SessionError::EventCountOverflow);
        };
        *slot = slot
            .checked_add(1)
            .ok_or(SessionError::EventCountOverflow)?;
    }
    let input_events =
        u16::try_from(batch.events.len()).map_err(|_| SessionError::EventCountOverflow)?;
    Ok(FrameEvidence {
        receipt_ns: batch.receipt_ns,
        input_events,
        source_events: counts,
        source_clock_regressions: batch.source_clock_regressions,
    })
}

#[cfg(test)]
mod profile_limit_tests {
    use super::{
        flush_receipt_group, push_event, session_cursor_new, session_cursor_new_with_limit,
        session_cursor_new_with_profile_limits, session_total_events, SessionError,
        MAX_EVENTS_PER_RECEIPT, MAX_RECORDED_PROFILE_EVENTS, RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT,
    };
    use crate::input::{EventKind, Identity, NormalizedEvent, SourceKind};

    fn same_receipt_event(sequence: u64) -> NormalizedEvent {
        event_at(sequence, 1_800_000_000_000_000_000)
    }

    fn event_at(sequence: u64, receipt_ns: i64) -> NormalizedEvent {
        NormalizedEvent {
            receipt_ns,
            input_receipt_ns: receipt_ns,
            source_ns: receipt_ns,
            sequence,
            source_message_id: None,
            source: SourceKind::Surveillance,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: 0xabc123,
            },
            centre: *b"ZAU1",
            kind: EventKind::Position {
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
                altitude_ft: Some(10_000),
                east_mmps: 0,
                north_mmps: 0,
                vertical_rate_fpm: None,
                track_callsign: None,
            },
        }
    }

    // Kills bypassing the raw receipt through an issue cutoff outside the sealed span.
    // REQ: OA-REG-013, OA-SEAL-014
    #[test]
    fn raw_receipt_inside_public_sealed_window_refuses_before_issue_cutoff() {
        let second_ns = 1_000_000_000i64;
        let candidate = oa_sealed::sealed_windows().iter().find_map(|window| {
            let input_receipt_ns = window.to_ns.checked_sub(1)?;
            let source_ns = window.from_ns.checked_sub(1)?;
            let issue_receipt_ns = input_receipt_ns
                .checked_add(second_ns - 1)?
                .checked_div(second_ns)?
                .checked_mul(second_ns)?;
            (source_ns >= 0
                && oa_sealed::in_sealed_window(input_receipt_ns)
                && !oa_sealed::in_sealed_window(source_ns)
                && !oa_sealed::in_sealed_window(issue_receipt_ns))
            .then_some((source_ns, input_receipt_ns, issue_receipt_ns))
        });
        let (source_ns, input_receipt_ns, issue_receipt_ns) = candidate.unwrap();
        let mut event = event_at(1, issue_receipt_ns);
        event.input_receipt_ns = input_receipt_ns;
        event.source_ns = source_ns;
        let mut cursor = session_cursor_new();
        assert_eq!(
            push_event(&mut cursor, event),
            Err(SessionError::SealedTimestamp)
        );
        assert_eq!(session_total_events(&cursor), 0);
    }

    #[test]
    fn offline_cursor_accepts_declared_cap_and_refuses_out_of_range_caps() {
        assert!(session_cursor_new_with_limit(MAX_RECORDED_PROFILE_EVENTS).is_ok());
        assert!(matches!(
            session_cursor_new_with_limit(0),
            Err(SessionError::SessionFull)
        ));
        assert!(matches!(
            session_cursor_new_with_limit(MAX_RECORDED_PROFILE_EVENTS + 1),
            Err(SessionError::SessionFull)
        ));
    }

    #[test]
    fn recorded_hour_receipt_cap_is_explicit_bounded_and_profile_specific() -> Result<(), String> {
        let mut ordinary = session_cursor_new_with_limit(10_000)
            .map_err(|error| format!("ordinary cursor limit is valid: {error:?}"))?;
        for sequence in 1..=MAX_EVENTS_PER_RECEIPT as u64 {
            push_event(&mut ordinary, same_receipt_event(sequence)).map_err(|error| {
                format!("ordinary receipt group remains within its cap: {error:?}")
            })?;
        }
        assert!(matches!(
            push_event(
                &mut ordinary,
                same_receipt_event(MAX_EVENTS_PER_RECEIPT as u64 + 1)
            ),
            Err(SessionError::ReceiptGroupFull)
        ));

        let mut hour =
            session_cursor_new_with_profile_limits(10_000, RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT)
                .map_err(|error| format!("recorded-hour cursor limit is valid: {error:?}"))?;
        for sequence in 1..=RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT as u64 {
            push_event(&mut hour, same_receipt_event(sequence)).map_err(|error| {
                format!("recorded-hour receipt group remains within its cap: {error:?}")
            })?;
        }
        assert!(matches!(
            push_event(
                &mut hour,
                same_receipt_event(RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT as u64 + 1)
            ),
            Err(SessionError::ReceiptGroupFull)
        ));
        assert!(matches!(
            session_cursor_new_with_profile_limits(
                10_000,
                RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT + 1
            ),
            Err(SessionError::ReceiptGroupFull)
        ));
        assert!(matches!(
            session_cursor_new_with_profile_limits(10_000, 512),
            Err(SessionError::ReceiptGroupFull)
        ));

        let dense_batch = super::ReceiptBatch {
            receipt_ns: 1_800_000_000_000_000_000,
            source_clock_regressions: 0,
            events: (1..=MAX_EVENTS_PER_RECEIPT + 1)
                .map(|sequence| same_receipt_event(sequence as u64))
                .collect(),
        };
        assert!(matches!(
            super::summarize_input_batch(&dense_batch),
            Err(SessionError::ReceiptGroupFull)
        ));
        assert_eq!(
            super::summarize_input_batch_with_limit(
                &dense_batch,
                RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT
            )
            .map_err(|error| format!("recorded-hour batch fits its declared event cap: {error:?}"))?
            .input_events,
            (MAX_EVENTS_PER_RECEIPT + 1) as u16
        );
        Ok(())
    }

    #[test]
    fn recorded_hour_batches_compact_at_rollover_and_eof_then_refill() -> Result<(), String> {
        let start = 1_800_000_000_000_000_000;
        let second = 1_000_000_000;
        let mut hour =
            session_cursor_new_with_profile_limits(32, RECORDED_HOUR_MAX_EVENTS_PER_RECEIPT)
                .map_err(|error| format!("recorded-hour cursor limit is valid: {error:?}"))?;
        assert!(push_event(&mut hour, event_at(1, start))
            .map_err(|error| format!("first event is admitted: {error:?}"))?
            .is_none());
        assert!(push_event(&mut hour, event_at(2, start))
            .map_err(|error| format!("same-receipt event is admitted: {error:?}"))?
            .is_none());
        let first = push_event(&mut hour, event_at(3, start + second))
            .map_err(|error| format!("receipt rollover succeeds: {error:?}"))?
            .ok_or("rollover flushes the first batch")?;
        assert_eq!(first.events.capacity(), first.events.len());
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        assert!(push_event(&mut hour, event_at(4, start + second))
            .map_err(|error| format!("same-receipt event is admitted: {error:?}"))?
            .is_none());
        let second_batch = push_event(&mut hour, event_at(5, start + 2 * second))
            .map_err(|error| format!("receipt rollover succeeds: {error:?}"))?
            .ok_or("rollover flushes the second batch")?;
        assert_eq!(second_batch.events.capacity(), second_batch.events.len());
        assert_eq!(
            second_batch
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );

        assert!(push_event(&mut hour, event_at(6, start + 2 * second))
            .map_err(|error| format!("same-receipt event is admitted: {error:?}"))?
            .is_none());
        let final_batch = flush_receipt_group(&mut hour)
            .map_err(|error| format!("final receipt flush succeeds: {error:?}"))?
            .ok_or("EOF flushes the final batch")?;
        assert_eq!(final_batch.events.capacity(), final_batch.events.len());
        assert_eq!(
            final_batch
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![5, 6]
        );

        let mut ordinary = session_cursor_new();
        assert!(push_event(&mut ordinary, event_at(1, start))
            .map_err(|error| format!("ordinary cursor accepts its first event: {error:?}"))?
            .is_none());
        let ordinary_batch = push_event(&mut ordinary, event_at(2, start + second))
            .map_err(|error| format!("ordinary cursor accepts next receipt: {error:?}"))?
            .ok_or("next receipt flushes the ordinary batch")?;
        assert_eq!(ordinary_batch.events.capacity(), MAX_EVENTS_PER_RECEIPT);
        Ok(())
    }
}

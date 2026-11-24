//! Bounded receipt-time event staging for the regional session. Shell (B1, B3, B7).
//!
//! One receipt-time group is held until a later timestamp or EOF establishes its boundary. The
//! later event is not applied until the previous group is returned, so every consumer sees only
//! the admitted prefix. Older source timestamps are retained for downstream stale-data rules.

use crate::input::{NormalizedEvent, SourceKind};

/// Maximum events sharing one receipt timestamp. Reaching the bound refuses the next row and
/// reports it; no event is silently replaced or dropped (RULESET C2, C7, E6).
pub const MAX_EVENTS_PER_RECEIPT: usize = 256;

/// Maximum normalized input events accepted by one invocation.
pub const MAX_SESSION_EVENTS: usize = 100_000;

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
        closed: false,
    }
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

/// Admits one normalized event and closes the prior receipt group when the timestamp advances.
/// The incoming event remains pending and is not part of the returned batch.
pub fn push_event(
    cursor: &mut SessionCursor,
    event: NormalizedEvent,
) -> Result<Option<ReceiptBatch>, SessionError> {
    if cursor.closed {
        return Err(SessionError::CursorClosed);
    }
    if event.receipt_ns < 0 || event.source_ns < 0 || event.source_ns > event.receipt_ns {
        return Err(SessionError::InvalidTime);
    }
    if oa_sealed::in_sealed_window(event.source_ns) || oa_sealed::in_sealed_window(event.receipt_ns)
    {
        return Err(SessionError::SealedTimestamp);
    }
    if cursor
        .prior_receipt_ns
        .is_some_and(|prior| event.receipt_ns < prior)
    {
        return Err(SessionError::ReceiptRegression);
    }
    if cursor.total_events >= MAX_SESSION_EVENTS as u64 {
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
        if cursor.pending.len() >= MAX_EVENTS_PER_RECEIPT {
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
                Vec::with_capacity(MAX_EVENTS_PER_RECEIPT),
            );
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
        Vec::with_capacity(MAX_EVENTS_PER_RECEIPT),
    );
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
    if batch.events.len() > MAX_EVENTS_PER_RECEIPT {
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

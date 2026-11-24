#![allow(clippy::expect_used)]

use oa_regional_session::{
    flush_receipt_group, parse_event_line, push_event, session_cursor_new, summarize_input_batch,
    ReceiptBatch, SessionError, MAX_EVENTS_PER_RECEIPT,
};

fn row(time: i64, source: &str, seq: u64, source_ns: i64) -> String {
    format!(
        "v1\t{time}\t{source_ns}\t{seq}\t{source}\t00000000000000000000000000000385\tABC123\tZAU1\tFEED\tUP\t1"
    )
}

#[test]
fn closes_only_the_previous_receipt_group_and_retains_old_source_stamp() {
    let mut cursor = session_cursor_new();
    let first = parse_event_line(&row(1_000, "ADAPTATION", 1, 900)).expect("valid row");
    assert!(push_event(&mut cursor, first).expect("admission").is_none());

    let delayed = parse_event_line(&row(2_000, "ADAPTATION", 2, 800)).expect("valid delayed row");
    let previous = push_event(&mut cursor, delayed).expect("delayed source stamp stays visible");
    let previous = previous.expect("new receipt time closes the old group");
    assert_eq!(previous.receipt_ns, 1_000);
    assert_eq!(previous.events.len(), 1);
    assert_eq!(previous.source_clock_regressions, 0);

    let final_batch = flush_receipt_group(&mut cursor)
        .expect("flush")
        .expect("final group");
    assert_eq!(final_batch.receipt_ns, 2_000);
    assert_eq!(final_batch.source_clock_regressions, 1);
}

#[test]
fn rejects_receipt_regression_and_duplicate_source_sequence() {
    let mut cursor = session_cursor_new();
    let first = parse_event_line(&row(2_000, "ADAPTATION", 1, 1_500)).expect("valid row");
    push_event(&mut cursor, first).expect("admission");
    let back = parse_event_line(&row(1_000, "ADAPTATION", 2, 900)).expect("valid row");
    assert_eq!(
        push_event(&mut cursor, back),
        Err(SessionError::ReceiptRegression)
    );
    let duplicate = parse_event_line(&row(3_000, "ADAPTATION", 1, 2_900)).expect("valid row");
    assert_eq!(
        push_event(&mut cursor, duplicate),
        Err(SessionError::SequenceRegression)
    );
}

#[test]
fn flush_closes_cursor_and_repeated_flush_is_idempotent() {
    let mut cursor = session_cursor_new();
    let event = parse_event_line(&row(1_000, "ADAPTATION", 1, 900)).expect("valid row");
    push_event(&mut cursor, event).expect("admission");
    assert!(flush_receipt_group(&mut cursor).expect("flush").is_some());
    assert!(flush_receipt_group(&mut cursor)
        .expect("repeated flush")
        .is_none());

    let same_receipt = parse_event_line(&row(1_000, "ADAPTATION", 2, 950)).expect("valid row");
    assert_eq!(
        push_event(&mut cursor, same_receipt),
        Err(SessionError::CursorClosed)
    );
}

#[test]
fn public_cursor_rejects_negative_and_future_source_times() {
    let mut cursor = session_cursor_new();
    let mut negative = parse_event_line(&row(1_000, "ADAPTATION", 1, 900)).expect("valid row");
    negative.receipt_ns = -1;
    assert_eq!(
        push_event(&mut cursor, negative),
        Err(SessionError::InvalidTime)
    );

    let mut future = parse_event_line(&row(1_000, "ADAPTATION", 1, 900)).expect("valid row");
    future.source_ns = 1_001;
    assert_eq!(
        push_event(&mut cursor, future),
        Err(SessionError::InvalidTime)
    );
}

#[test]
fn public_batch_summary_rejects_more_than_the_receipt_cap() {
    let event = parse_event_line(&row(1_000, "ADAPTATION", 1, 900)).expect("valid row");
    let batch = ReceiptBatch {
        receipt_ns: 1_000,
        source_clock_regressions: 0,
        events: vec![event; MAX_EVENTS_PER_RECEIPT + 1],
    };
    assert_eq!(
        summarize_input_batch(&batch),
        Err(SessionError::ReceiptGroupFull)
    );
}

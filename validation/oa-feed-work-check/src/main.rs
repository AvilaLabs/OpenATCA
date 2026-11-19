#![forbid(unsafe_code)]
//! Standalone deterministic service-turn fixture for the oa-feed FrameReader. Shell-only:
//! builds in-memory bytes, measures bounded service calls with Instant, and reports plain values.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use oa_feed::{
    encode_frame, encode_frame_mono, frame_work_refusal_retryable, ErrorKind, FrameReader,
    FrameWorkBudget, FrameWorkRefusal, FrameWorkStats, HEADER_LEN, HEADER_LEN_V2, MAX_FRAME_LEN,
    MAX_PAYLOAD, TRAILER_LEN,
};

const SERVICE_TURN_LIMIT: Duration = Duration::from_secs(1);
const MAX_SERVICE_TURNS_PER_CASE: u32 = 8;
const READER_CAPACITY: usize = 2 * MAX_FRAME_LEN;
const OVERLAP: usize = HEADER_LEN_V2;

#[derive(Clone, Copy)]
// A7.2: cold fixture expectation record.
struct Expected {
    frame_sequences: &'static [u64],
    frames: u64,
    frame_errors: u64,
    framed_bytes: u64,
    skipped_bytes: u64,
    retained_before_finish: u64,
    crc_bytes: u64,
    scan_positions: u64,
    compacted_bytes: u64,
    refusals: u64,
}

// A7.2: one cold aggregate for a standalone measurement fixture.
struct Totals {
    accepted_bytes: u64,
    framed_bytes: u64,
    frames: u64,
    frame_errors: u64,
    max_turn_ns: u128,
    finalize_ns: u128,
    turns: u32,
    refusals: u64,
    max_scan_positions: u64,
    max_crc_bytes: u64,
    max_copied_bytes: u64,
    max_compacted_bytes: u64,
}

impl Totals {
    const fn new() -> Self {
        Self {
            accepted_bytes: 0,
            framed_bytes: 0,
            frames: 0,
            frame_errors: 0,
            max_turn_ns: 0,
            finalize_ns: 0,
            turns: 0,
            refusals: 0,
            max_scan_positions: 0,
            max_crc_bytes: 0,
            max_copied_bytes: 0,
            max_compacted_bytes: 0,
        }
    }
}

// A7.2: cold shell report row.
struct ResultRow {
    name: &'static str,
    classification: &'static str,
    offered_bytes: u64,
    accepted_bytes: u64,
    framed_bytes: u64,
    skipped_bytes: u64,
    retained_before_finish: u64,
    retained_bytes: u64,
    unoffered_bytes: u64,
    frame_errors: u64,
    work: FrameWorkStats,
    totals: Totals,
}

fn main() -> ExitCode {
    println!(
        "EXPERIMENTAL_SERVICE_TURN_LIMIT_NS={} scope=fixture-only; no universal hard-real-time claim",
        SERVICE_TURN_LIMIT.as_nanos()
    );
    match run_all(true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL_OA_FEED_WORK: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `enforce_time` applies the wall-clock service-turn limit; the unit test passes `false` and
/// relies on the deterministic bounds (turn cap, refusal progress, byte conservation).
fn run_all(enforce_time: bool) -> Result<(), String> {
    let valid = maximum_frame(false)?;
    let valid_work = (MAX_FRAME_LEN - TRAILER_LEN) as u64;
    let valid_row = run_fixture(
        "maximum-valid-frame",
        "valid-capacity",
        valid,
        MAX_FRAME_LEN,
        Expected {
            frame_sequences: &[1],
            frames: 1,
            frame_errors: 0,
            framed_bytes: to_u64(MAX_FRAME_LEN)?,
            skipped_bytes: 0,
            retained_before_finish: 0,
            crc_bytes: valid_work,
            scan_positions: 0,
            compacted_bytes: 0,
            refusals: 0,
        },
        enforce_time,
    )?;
    print_row(&valid_row);

    let corrupt = maximum_frame(true)?;
    let corrupt_scan = (MAX_FRAME_LEN - 4) as u64;
    let corrupt_row = run_fixture(
        "maximum-corrupt-resync",
        "malformed-safety",
        corrupt,
        MAX_FRAME_LEN,
        Expected {
            frame_sequences: &[],
            frames: 0,
            frame_errors: 1,
            framed_bytes: 0,
            skipped_bytes: to_u64(MAX_FRAME_LEN)?,
            retained_before_finish: 3,
            crc_bytes: valid_work,
            scan_positions: corrupt_scan,
            compacted_bytes: 0,
            refusals: 0,
        },
        enforce_time,
    )?;
    print_row(&corrupt_row);

    let overlap = overlapping_corrupt_frames()?;
    let tail_len = HEADER_LEN + b"tail".len() + TRAILER_LEN;
    let overlap_tail_start = overlap
        .len()
        .checked_sub(tail_len)
        .ok_or_else(|| "overlap tail length exceeds fixture".to_string())?;
    let tail_crc = to_u64(HEADER_LEN + b"tail".len())?;
    let overlap_crc = valid_work
        .checked_mul(2)
        .and_then(|n| n.checked_add(tail_crc))
        .ok_or_else(|| "overlap expected CRC total overflow".to_string())?;
    let overlap_row = run_fixture(
        "overlapping-corrupt-defer-resume",
        "malformed-safety-with-valid-tail",
        overlap,
        READER_CAPACITY,
        Expected {
            frame_sequences: &[9],
            frames: 1,
            frame_errors: 2,
            framed_bytes: to_u64(tail_len)?,
            skipped_bytes: to_u64(overlap_tail_start)?,
            retained_before_finish: 0,
            crc_bytes: overlap_crc,
            scan_positions: (MAX_FRAME_LEN + OVERLAP) as u64,
            compacted_bytes: 0,
            refusals: 2,
        },
        enforce_time,
    )?;
    print_row(&overlap_row);

    let (compaction, compacted_expected, compact_crc) = compaction_stream()?;
    let compaction_bytes = to_u64(compaction.len())?;
    let compaction_row = run_fixture(
        "compaction-and-service-resume",
        "valid-service-resume",
        compaction,
        MAX_FRAME_LEN,
        Expected {
            frame_sequences: &[1, 2, 3],
            frames: 3,
            frame_errors: 0,
            framed_bytes: compaction_bytes,
            skipped_bytes: 0,
            retained_before_finish: 0,
            crc_bytes: compact_crc,
            scan_positions: 0,
            compacted_bytes: compacted_expected,
            refusals: 2,
        },
        enforce_time,
    )?;
    print_row(&compaction_row);

    let source_bytes = valid_row
        .offered_bytes
        .checked_add(corrupt_row.offered_bytes)
        .and_then(|n| n.checked_add(overlap_row.offered_bytes))
        .and_then(|n| n.checked_add(compaction_row.offered_bytes))
        .ok_or_else(|| "fixture source-byte total overflow".to_string())?;
    let turns = valid_row
        .totals
        .turns
        .checked_add(corrupt_row.totals.turns)
        .and_then(|n| n.checked_add(overlap_row.totals.turns))
        .and_then(|n| n.checked_add(compaction_row.totals.turns))
        .ok_or_else(|| "fixture service-turn total overflow".to_string())?;
    let max_turn_ns = valid_row
        .totals
        .max_turn_ns
        .max(corrupt_row.totals.max_turn_ns)
        .max(overlap_row.totals.max_turn_ns)
        .max(compaction_row.totals.max_turn_ns);
    if turns > MAX_SERVICE_TURNS_PER_CASE * 4 {
        return Err(format!(
            "service turns {turns} exceeded the finite run limit"
        ));
    }
    println!(
        "PASS_OA_FEED_WORK source_bytes={source_bytes} service_turns={turns} max_turn_ns={max_turn_ns} declared_limit_ns={}",
        SERVICE_TURN_LIMIT.as_nanos()
    );
    Ok(())
}

fn run_fixture(
    name: &'static str,
    classification: &'static str,
    source: Vec<u8>,
    capacity: usize,
    expected: Expected,
    enforce_time: bool,
) -> Result<ResultRow, String> {
    let offered_bytes = to_u64(source.len())?;
    let mut storage = vec![0u8; capacity];
    let mut reader = FrameReader::new(&mut storage)
        .map_err(|error| format!("{name}: reader buffer refused: {error:?}"))?;
    let mut cursor = 0usize;
    let mut totals = Totals::new();

    loop {
        if totals.turns >= MAX_SERVICE_TURNS_PER_CASE {
            return Err(format!("{name}: exhausted finite service-turn limit"));
        }
        let before = reader.work_stats();
        let before_offset = reader.offset();
        let before_cursor = cursor;
        let before_buffered = reader.buffered_len();
        let mut budget = FrameWorkBudget::for_capacity(capacity);
        let started = Instant::now();
        let refusal = service_turn(
            &mut reader,
            &source,
            &mut cursor,
            &mut budget,
            &mut totals,
            name,
            expected.frame_sequences,
        )?;
        let elapsed = started.elapsed();
        let after = reader.work_stats();
        let turn_scan = after.scan_positions.saturating_sub(before.scan_positions);
        let turn_crc = after.crc_bytes.saturating_sub(before.crc_bytes);
        let turn_copy = after.copied_bytes.saturating_sub(before.copied_bytes);
        let turn_compact = after.compacted_bytes.saturating_sub(before.compacted_bytes);
        totals.max_turn_ns = totals.max_turn_ns.max(elapsed.as_nanos());
        totals.max_scan_positions = totals.max_scan_positions.max(turn_scan);
        totals.max_crc_bytes = totals.max_crc_bytes.max(turn_crc);
        totals.max_copied_bytes = totals.max_copied_bytes.max(turn_copy);
        totals.max_compacted_bytes = totals.max_compacted_bytes.max(turn_compact);
        totals.turns += 1;
        if enforce_time && elapsed >= SERVICE_TURN_LIMIT {
            return Err(format!(
                "{name}: service turn {} ns exceeded declared {} ns experimental limit",
                elapsed.as_nanos(),
                SERVICE_TURN_LIMIT.as_nanos()
            ));
        }
        if let Some(refusal) = refusal {
            if !frame_work_refusal_retryable(refusal, FrameWorkBudget::for_capacity(capacity)) {
                return Err(format!("{name}: nonrenewable work refusal {refusal:?}"));
            }
            if cursor == before_cursor
                && reader.offset() == before_offset
                && reader.buffered_len() == before_buffered
                && after == before
            {
                return Err(format!(
                    "{name}: retryable refusal made no state progress: {refusal:?}"
                ));
            }
            totals.refusals += 1;
        } else if cursor == source.len() {
            break;
        } else if cursor == before_cursor
            && reader.offset() == before_offset
            && reader.buffered_len() == before_buffered
            && after == before
            && cursor < source.len()
        {
            return Err(format!(
                "{name}: service turn made no progress without a refusal"
            ));
        }
    }

    let unoffered_bytes = to_u64(source.len().saturating_sub(cursor))?;
    let retained_before_finish = to_u64(reader.buffered_len())?;
    let accepted_before_finish = to_u64(cursor)?;
    let owned_before_finish = totals
        .framed_bytes
        .checked_add(reader.skipped_total())
        .and_then(|n| n.checked_add(retained_before_finish))
        .ok_or_else(|| format!("{name}: pre-finish ownership sum overflow"))?;
    if accepted_before_finish != owned_before_finish {
        return Err(format!(
            "{name}: pre-finish accepted={accepted_before_finish}, framed={}, skipped={}, retained={retained_before_finish}",
            totals.framed_bytes,
            reader.skipped_total()
        ));
    }
    let finish_started = Instant::now();
    if reader.finish().is_some() {
        totals.frame_errors += 1;
    }
    let finish_elapsed = finish_started.elapsed();
    totals.finalize_ns = finish_elapsed.as_nanos();
    if enforce_time && finish_elapsed >= SERVICE_TURN_LIMIT {
        return Err(format!(
            "{name}: finalization exceeded declared service-turn limit"
        ));
    }
    let work = reader.work_stats();
    let skipped_bytes = reader.skipped_total();
    let retained_bytes = to_u64(reader.buffered_len())?;
    if cursor != source.len() || retained_bytes != 0 {
        return Err(format!(
            "{name}: unresolved ownership cursor={cursor}/{} retained={retained_bytes}",
            source.len()
        ));
    }
    let accepted_bytes = to_u64(cursor)?;
    let resolved = totals
        .framed_bytes
        .checked_add(skipped_bytes)
        .and_then(|n| n.checked_add(retained_bytes))
        .ok_or_else(|| format!("{name}: ownership sum overflow"))?;
    let offered_check = accepted_bytes
        .checked_add(unoffered_bytes)
        .ok_or_else(|| format!("{name}: offered-byte sum overflow"))?;
    if accepted_bytes != resolved
        || offered_bytes != offered_check
        || totals.accepted_bytes != accepted_bytes
    {
        return Err(format!(
            "{name}: conservation failed offered={offered_bytes} accepted={accepted_bytes} framed={} skipped={skipped_bytes} retained={retained_bytes} unoffered={unoffered_bytes}",
            totals.framed_bytes
        ));
    }
    if totals.refusals != expected.refusals
        || totals.frame_errors != expected.frame_errors
        || totals.framed_bytes != expected.framed_bytes
        || skipped_bytes != expected.skipped_bytes
        || retained_before_finish != expected.retained_before_finish
        || work.crc_bytes != expected.crc_bytes
        || work.scan_positions != expected.scan_positions
        || work.copied_bytes != accepted_bytes
        || work.compacted_bytes != expected.compacted_bytes
        || totals.frames != expected.frames
    {
        return Err(format!(
            "{name}: expected frames={} errors={} framed={} skipped={} retained={} refusals={} crc={} scan={} compact={}, got frames={} errors={} framed={} skipped={} retained={} refusals={} crc={} scan={} compact={}",
            expected.frames,
            expected.frame_errors,
            expected.framed_bytes,
            expected.skipped_bytes,
            expected.retained_before_finish,
            expected.refusals,
            expected.crc_bytes,
            expected.scan_positions,
            expected.compacted_bytes,
            totals.frames,
            totals.frame_errors,
            totals.framed_bytes,
            skipped_bytes,
            retained_before_finish,
            totals.refusals,
            work.crc_bytes,
            work.scan_positions,
            work.compacted_bytes
        ));
    }
    if totals.max_scan_positions > to_u64(capacity)?
        || totals.max_crc_bytes > to_u64(MAX_FRAME_LEN)?
        || totals.max_copied_bytes > to_u64(capacity)?
        || totals.max_compacted_bytes > to_u64(capacity)?
    {
        return Err(format!(
            "{name}: observed per-turn work exceeded its profile"
        ));
    }
    Ok(ResultRow {
        name,
        classification,
        offered_bytes,
        accepted_bytes,
        framed_bytes: totals.framed_bytes,
        skipped_bytes,
        retained_before_finish,
        retained_bytes,
        unoffered_bytes,
        frame_errors: totals.frame_errors,
        work,
        totals,
    })
}

fn service_turn(
    reader: &mut FrameReader<'_>,
    source: &[u8],
    cursor: &mut usize,
    budget: &mut FrameWorkBudget,
    totals: &mut Totals,
    name: &str,
    expected_sequences: &[u64],
) -> Result<Option<FrameWorkRefusal>, String> {
    loop {
        if *cursor < source.len() {
            let Some(rest) = source.get(*cursor..) else {
                return Err(format!("{name}: source cursor outside offered bytes"));
            };
            let taken = match reader.push_with_remaining_budget(rest, budget) {
                Ok(taken) => taken,
                Err(refusal) => return Ok(Some(refusal)),
            };
            if taken == 0 {
                return Err(format!("{name}: push made no progress"));
            }
            *cursor = (*cursor)
                .checked_add(taken)
                .ok_or_else(|| format!("{name}: source cursor overflow"))?;
            totals.accepted_bytes = totals
                .accepted_bytes
                .checked_add(to_u64(taken)?)
                .ok_or_else(|| format!("{name}: accepted-byte counter overflow"))?;
        }
        loop {
            match reader.next_with_remaining_budget(budget) {
                Ok(Some(Ok(frame))) => {
                    let ordinal = usize::try_from(totals.frames)
                        .map_err(|_| format!("{name}: frame ordinal exceeds usize"))?;
                    if expected_sequences.get(ordinal) != Some(&frame.seq) {
                        return Err(format!(
                            "{name}: unexpected sequence {} at ordinal {ordinal}",
                            frame.seq
                        ));
                    }
                    let start = usize::try_from(frame.offset)
                        .map_err(|_| format!("{name}: source frame offset exceeds usize"))?;
                    let end = start
                        .checked_add(frame.raw.len())
                        .ok_or_else(|| format!("{name}: source frame range overflow"))?;
                    if source.get(start..end) != Some(frame.raw) {
                        return Err(format!(
                            "{name}: returned frame differs from offered source bytes"
                        ));
                    }
                    totals.frames = totals
                        .frames
                        .checked_add(1)
                        .ok_or_else(|| format!("{name}: frame counter overflow"))?;
                    totals.framed_bytes = totals
                        .framed_bytes
                        .checked_add(to_u64(frame.raw.len())?)
                        .ok_or_else(|| format!("{name}: framed-byte counter overflow"))?;
                }
                Ok(Some(Err(error))) => {
                    totals.frame_errors = totals
                        .frame_errors
                        .checked_add(1)
                        .ok_or_else(|| format!("{name}: frame-error counter overflow"))?;
                    if !matches!(error.kind, ErrorKind::Crc { .. }) {
                        return Err(format!("{name}: unexpected frame error {error:?}"));
                    }
                }
                Ok(None) => break,
                Err(refusal) => return Ok(Some(refusal)),
            }
        }
        if *cursor == source.len() {
            return Ok(None);
        }
    }
}

fn maximum_frame(corrupt: bool) -> Result<Vec<u8>, String> {
    let payload = vec![0x5au8; MAX_PAYLOAD];
    let mut frame = vec![0u8; MAX_FRAME_LEN];
    let written = encode_frame_mono(&mut frame, 1, 0, 1, 1, 0, 7, &payload)
        .ok_or_else(|| "maximum frame encoding refused exact-capacity fixture".to_string())?;
    if written != MAX_FRAME_LEN {
        return Err(format!("maximum frame length {written} != {MAX_FRAME_LEN}"));
    }
    if corrupt {
        let last = frame
            .last_mut()
            .ok_or_else(|| "maximum frame unexpectedly empty".to_string())?;
        *last ^= 1;
    }
    Ok(frame)
}

fn overlapping_corrupt_frames() -> Result<Vec<u8>, String> {
    let tail_payload = b"tail";
    let mut tail = vec![0u8; HEADER_LEN + tail_payload.len() + TRAILER_LEN];
    let tail_len = encode_frame(&mut tail, 1, 0, 9, 3, 0, tail_payload)
        .ok_or_else(|| "tail frame encoding refused exact-capacity fixture".to_string())?;
    tail.truncate(tail_len);
    let total = MAX_FRAME_LEN
        .checked_add(OVERLAP)
        .and_then(|n| n.checked_add(tail.len()))
        .ok_or_else(|| "overlap fixture length overflow".to_string())?;
    let mut source = vec![0u8; total];
    let payload = vec![0x37u8; MAX_PAYLOAD];
    let first = encode_frame_mono(
        source
            .get_mut(..MAX_FRAME_LEN)
            .ok_or_else(|| "first overlap frame slice missing".to_string())?,
        1,
        0,
        1,
        1,
        0,
        4,
        &payload,
    )
    .ok_or_else(|| "first overlap frame encoding refused".to_string())?;
    let second_end = OVERLAP
        .checked_add(MAX_FRAME_LEN)
        .ok_or_else(|| "second overlap end overflow".to_string())?;
    let second = encode_frame_mono(
        source
            .get_mut(OVERLAP..second_end)
            .ok_or_else(|| "second overlap frame slice missing".to_string())?,
        1,
        0,
        2,
        2,
        0,
        5,
        &payload,
    )
    .ok_or_else(|| "second overlap frame encoding refused".to_string())?;
    if first != MAX_FRAME_LEN || second != MAX_FRAME_LEN {
        return Err("overlap fixture produced unexpected frame lengths".to_string());
    }
    let tail_start = second_end;
    let tail_dst = source
        .get_mut(tail_start..)
        .ok_or_else(|| "valid tail destination missing".to_string())?;
    tail_dst.copy_from_slice(&tail);
    let damaged = source
        .get_mut(MAX_FRAME_LEN - 1)
        .ok_or_else(|| "overlap corruption byte missing".to_string())?;
    *damaged ^= 1;
    Ok(source)
}

fn compaction_stream() -> Result<(Vec<u8>, u64, u64), String> {
    let first_payload = b"first";
    let mut first = vec![0u8; HEADER_LEN + first_payload.len() + TRAILER_LEN];
    let first_len = encode_frame(&mut first, 1, 0, 1, 1, 0, first_payload)
        .ok_or_else(|| "first compaction frame encoding refused".to_string())?;
    first.truncate(first_len);
    let last_payload = b"last";
    let mut last = vec![0u8; HEADER_LEN + last_payload.len() + TRAILER_LEN];
    let last_len = encode_frame(&mut last, 1, 0, 3, 3, 0, last_payload)
        .ok_or_else(|| "last compaction frame encoding refused".to_string())?;
    last.truncate(last_len);
    let middle_payload = vec![0x29u8; MAX_PAYLOAD];
    let mut middle = vec![0u8; MAX_FRAME_LEN];
    let middle_len = encode_frame_mono(&mut middle, 1, 0, 2, 2, 0, 6, &middle_payload)
        .ok_or_else(|| "middle maximum frame encoding refused".to_string())?;
    if middle_len != MAX_FRAME_LEN {
        return Err("middle compaction frame length was not maximum".to_string());
    }
    let compacted = MAX_FRAME_LEN
        .checked_sub(first.len())
        .ok_or_else(|| "compaction estimate underflow".to_string())?;
    let crc = (first.len() - TRAILER_LEN)
        .checked_add(MAX_FRAME_LEN - TRAILER_LEN)
        .and_then(|n| n.checked_add(last.len() - TRAILER_LEN))
        .ok_or_else(|| "compaction expected CRC total overflow".to_string())?;
    let mut source = Vec::with_capacity(first.len() + middle.len() + last.len());
    source.extend_from_slice(&first);
    source.extend_from_slice(&middle);
    source.extend_from_slice(&last);
    Ok((source, to_u64(compacted)?, to_u64(crc)?))
}

fn print_row(row: &ResultRow) {
    println!(
        "case={} class={} status=PASS turns={} renewed_deferrals={} offered={} accepted={} frames={} framed={} skipped={} retained_before_finalize={} retained_after_finalize={} unoffered={} frame_errors={} scan_positions={} crc_bytes={} copied_bytes={} compacted_bytes={} max_scan_positions={} max_crc_bytes={} max_copied_bytes={} max_compacted_bytes={} max_turn_ns={} finalize_ns={}",
        row.name,
        row.classification,
        row.totals.turns,
        row.totals.refusals,
        row.offered_bytes,
        row.accepted_bytes,
        row.totals.frames,
        row.framed_bytes,
        row.skipped_bytes,
        row.retained_before_finish,
        row.retained_bytes,
        row.unoffered_bytes,
        row.frame_errors,
        row.work.scan_positions,
        row.work.crc_bytes,
        row.work.copied_bytes,
        row.work.compacted_bytes,
        row.totals.max_scan_positions,
        row.totals.max_crc_bytes,
        row.totals.max_copied_bytes,
        row.totals.max_compacted_bytes,
        row.totals.max_turn_ns,
        row.totals.finalize_ns,
    );
}

fn to_u64(value: usize) -> Result<u64, String> {
    u64::try_from(value)
        .map_err(|_| "usize value does not fit the fixture's u64 counters".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-FEED-188
    #[test]
    fn bounded_fixtures_conserve_source_bytes_and_frame_identity() -> Result<(), String> {
        run_all(false)
    }
}

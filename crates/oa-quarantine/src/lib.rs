#![forbid(unsafe_code)]
#![no_std]
//! Bounded per-input quarantine bookkeeping (M7, C/H).
//!
//! This is a pure status/queue core. It copies no raw message payload, performs no I/O, reads no
//! clock, invokes no callbacks, and does not mutate a caller's accepted flight, track, or
//! adaptation state. The caller validates/stages an item, supplies its stable identity, refusal
//! reasons, measured work, and timestamps in one declared monotonic clock domain, then applies its
//! accepted state change only after [`route_input`] returns [`Disposition::Accepted`]. A refusal
//! is either queued for source-specific manual handling or returned immediately as
//! [`Disposition::ManualHandlingRequired`] when that source's queue is full. Fullness in one input
//! class does not consume another class's queue capacity.
//!
//! Queue rows identify the original item so a shell can retrieve it from its source's durable
//! intake record. They are not a copy of that source payload and do not themselves guarantee a
//! durable quarantine workflow, operator alert, or end-to-end isolation. The core's counters and
//! all caller-supplied identity, work, and time claims are unverified.
//!
//! Each of the three queues holds 128 metadata events. This is a bounded manual-triage backlog,
//! not a traffic buffer; the 129th pending refusal is returned synchronously as a counted manual
//! handling event. The 384 rows occupy 12 KiB, with less than 384 bytes of queue metadata. At a
//! design-rate envelope of 10,000 decisions/s for 15 years, even a reason bit set on every input
//! receives fewer than 4.3e13 increments; checked `u64` counters retain ample margin. This is a
//! sizing rationale, not a measured input-rate claim. Input validation is constant work; accounting
//! examines at most nine reason bits. All capacities and bounds are checked before mutation. There
//! is no allocator; its only dependency supplies plain audit event/receipt values. Durable log
//! access stays in the shell and occurs before a receipt-gated queue release.

/// Maximum queued events per independent input class; a manual-triage backlog, not traffic storage.
pub const MAX_PENDING_PER_SOURCE: usize = 128;
/// Input classes whose failures must remain isolated from one another.
pub const INPUT_SOURCE_COUNT: usize = 3;
/// Total fixed event slots.
pub const MAX_PENDING_EVENTS: usize = MAX_PENDING_PER_SOURCE * INPUT_SOURCE_COUNT;
/// Number of defined refusal-reason bits counted separately per input class.
pub const REASON_COUNTER_COUNT: usize = 9;
/// Sizing envelope used for the 15-year checked-counter justification; not a runtime rate claim.
pub const COUNTER_DESIGN_RATE_PER_SECOND: u64 = 10_000;
/// Sizing horizon used for long-lived counters; not a product-retention promise.
pub const COUNTER_DESIGN_HORIZON_YEARS: u64 = 15;
const DEFINED_REASON_MASK: u16 = (1u16 << REASON_COUNTER_COUNT) - 1;

// RULESET B10: records and source indexes use the fixed 64-bit layout below.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-quarantine supports 64-bit targets only");

/// Input class; separate partitions ensure one queue cannot starve another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InputSource {
    FlightData = 1,
    Surveillance = 2,
    Adaptation = 3,
}

/// Stable source-local item identity. The values are caller claims, not authenticated identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InputIdentity {
    pub source: InputSource,
    /// Stable source/producer identifier within the input class; zero is reserved.
    pub source_instance: u16,
    /// Stable item identifier within the source instance; zero is reserved.
    pub item_id: u64,
}

/// Refusal reasons. Multiple independent conditions can be recorded on one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ReasonFlags(pub u16);

pub const REASON_INPUT_INVALID: u16 = 1 << 0;
pub const REASON_RESOURCE_LIMIT: u16 = 1 << 1;
pub const REASON_WORK_BUDGET: u16 = 1 << 2;
pub const REASON_TIME_BUDGET: u16 = 1 << 3;
pub const REASON_ALTITUDE_ROUTE_INVALID: u16 = 1 << 4;
pub const REASON_WAYPOINT_NAME_AMBIGUOUS: u16 = 1 << 5;
pub const REASON_SURVEILLANCE_INVALID: u16 = 1 << 6;
pub const REASON_ADAPTATION_CONFLICT: u16 = 1 << 7;
pub const REASON_IDENTITY_OR_LOCATION_CONFLICT: u16 = 1 << 8;

/// Outcome supplied by an adapter after staging and validating one item.
///
/// All times are non-negative nanoseconds in `clock_domain_id`. `work_units` is an adapter-defined
/// deterministic operation count. This core checks reported values against the explicit bounds;
/// it cannot attest that the caller measured either quantity honestly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InputOutcome {
    pub identity: InputIdentity,
    pub refusal_reasons: ReasonFlags,
    pub clock_domain_id: u32,
    pub started_at_ns: i64,
    pub completed_at_ns: i64,
    pub elapsed_limit_ns: u64,
    pub work_units: u32,
    pub work_limit: u32,
}

/// One bounded event for source-local manual handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct QuarantineEvent {
    pub identity: InputIdentity,
    pub reasons: ReasonFlags,
    pub clock_domain_id: u32,
    pub completed_at_ns: i64,
}

/// Result of classifying an input. `Accepted` means only that the caller-supplied checks and
/// budgets passed; the caller still owns and performs its own state mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Accepted,
    Queued(QuarantineEvent),
    ManualHandlingRequired(QuarantineEvent),
}

/// Per-source accounting, including every reason bit even when a queue is full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SourceStats {
    pub accepted: u64,
    pub queued: u64,
    pub queue_full_manual_events: u64,
    pub acknowledged: u64,
    pub reason_counts: [u64; REASON_COUNTER_COUNT],
}

const EMPTY_STATS: SourceStats = SourceStats {
    accepted: 0,
    queued: 0,
    queue_full_manual_events: 0,
    acknowledged: 0,
    reason_counts: [0; REASON_COUNTER_COUNT],
};

/// Fixed-capacity queue subsystem; each source class owns a disjoint contiguous segment.
/// A7: hot subsystem state with an exact size guard; no pointer or allocator state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct QuarantineQueue {
    events: [QuarantineEvent; MAX_PENDING_EVENTS],
    heads: [u16; INPUT_SOURCE_COUNT],
    lengths: [u16; INPUT_SOURCE_COUNT],
    stats: [SourceStats; INPUT_SOURCE_COUNT],
    release_audit_checkpoint: oa_audit_core::Checkpoint,
}

// A7.1: each event is 32 bytes and the 384 slots are 12 KiB. Long-lived u64 counters are sized
// for 10,000 decisions/s and a 15-year envelope, including all nine reason bits per input.
// A7.1: retain the last consumed release sequence and chain tail to reject replay or wrong-chain
// receipts; these 40 bytes are cold audit metadata alongside the manual-triage queue.
const _: () = assert!(core::mem::size_of::<InputIdentity>() == 16);
const _: () = assert!(core::mem::size_of::<InputOutcome>() == 56);
const _: () = assert!(core::mem::size_of::<QuarantineEvent>() == 32);
const _: () = assert!(core::mem::size_of::<SourceStats>() == 104);
const _: () = assert!(core::mem::size_of::<QuarantineQueue>() == 12_656);

/// Refusal from this core; arithmetic and invariant failures are explicit systemic errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum QuarantineError {
    InvalidIdentity = 1,
    InvalidClockDomain = 2,
    InvalidTimestamp = 3,
    ClockRegression = 4,
    InvalidTimeLimit = 5,
    InvalidWorkLimit = 6,
    UnknownReasonBits = 7,
    CounterOverflow = 8,
    EmptyQueue = 9,
    CapacityInvariant = 10,
    AuditRequired = 11,
}

fn checked_ref<T>(slice: &[T], index: usize) -> Result<&T, QuarantineError> {
    slice.get(index).ok_or(QuarantineError::CapacityInvariant)
}

fn checked_mut<T>(slice: &mut [T], index: usize) -> Result<&mut T, QuarantineError> {
    slice
        .get_mut(index)
        .ok_or(QuarantineError::CapacityInvariant)
}

fn source_index(source: InputSource) -> usize {
    match source {
        InputSource::FlightData => 0,
        InputSource::Surveillance => 1,
        InputSource::Adaptation => 2,
    }
}

fn add_one(value: u64) -> Result<u64, QuarantineError> {
    value.checked_add(1).ok_or(QuarantineError::CounterOverflow)
}

fn empty_event() -> QuarantineEvent {
    QuarantineEvent {
        identity: InputIdentity {
            source: InputSource::FlightData,
            source_instance: 0,
            item_id: 0,
        },
        reasons: ReasonFlags(0),
        clock_domain_id: 0,
        completed_at_ns: 0,
    }
}

/// Initialize an empty fixed-capacity queue subsystem.
pub fn queue_new() -> QuarantineQueue {
    queue_new_with_audit_checkpoint(oa_audit_core::genesis())
}

/// Initialize an empty queue with the trusted checkpoint for its dedicated audit chain.
/// The shell still verifies this checkpoint against the log before an append.
pub fn queue_new_with_audit_checkpoint(checkpoint: oa_audit_core::Checkpoint) -> QuarantineQueue {
    QuarantineQueue {
        events: [empty_event(); MAX_PENDING_EVENTS],
        heads: [0; INPUT_SOURCE_COUNT],
        lengths: [0; INPUT_SOURCE_COUNT],
        stats: [EMPTY_STATS; INPUT_SOURCE_COUNT],
        release_audit_checkpoint: checkpoint,
    }
}

fn validated_event(outcome: InputOutcome) -> Result<Option<QuarantineEvent>, QuarantineError> {
    if outcome.identity.source_instance == 0 || outcome.identity.item_id == 0 {
        return Err(QuarantineError::InvalidIdentity);
    }
    if outcome.clock_domain_id == 0 {
        return Err(QuarantineError::InvalidClockDomain);
    }
    if outcome.started_at_ns < 0 || outcome.completed_at_ns < 0 {
        return Err(QuarantineError::InvalidTimestamp);
    }
    let elapsed = outcome
        .completed_at_ns
        .checked_sub(outcome.started_at_ns)
        .ok_or(QuarantineError::ClockRegression)?;
    if elapsed < 0 {
        return Err(QuarantineError::ClockRegression);
    }
    if outcome.elapsed_limit_ns == 0 {
        return Err(QuarantineError::InvalidTimeLimit);
    }
    if outcome.work_limit == 0 {
        return Err(QuarantineError::InvalidWorkLimit);
    }
    if outcome.refusal_reasons.0 & !DEFINED_REASON_MASK != 0 {
        return Err(QuarantineError::UnknownReasonBits);
    }

    let mut reasons = outcome.refusal_reasons.0;
    if outcome.work_units > outcome.work_limit {
        reasons |= REASON_WORK_BUDGET;
    }
    if u64::try_from(elapsed).map_err(|_| QuarantineError::ClockRegression)?
        > outcome.elapsed_limit_ns
    {
        reasons |= REASON_TIME_BUDGET;
    }
    if reasons == 0 {
        return Ok(None);
    }
    Ok(Some(QuarantineEvent {
        identity: outcome.identity,
        reasons: ReasonFlags(reasons),
        clock_domain_id: outcome.clock_domain_id,
        completed_at_ns: outcome.completed_at_ns,
    }))
}

fn updated_reason_counts(
    mut stats: SourceStats,
    reasons: u16,
) -> Result<SourceStats, QuarantineError> {
    let mut bit_index = 0usize;
    while bit_index < REASON_COUNTER_COUNT {
        let bit = 1u16
            .checked_shl(u32::try_from(bit_index).map_err(|_| QuarantineError::CapacityInvariant)?)
            .ok_or(QuarantineError::CapacityInvariant)?;
        if reasons & bit != 0 {
            let next_count = add_one(*checked_ref(&stats.reason_counts, bit_index)?)?;
            *checked_mut(&mut stats.reason_counts, bit_index)? = next_count;
        }
        bit_index += 1;
    }
    Ok(stats)
}

fn event_index(source_index: usize, head: u16, length: u16) -> Result<usize, QuarantineError> {
    if usize::from(length) >= MAX_PENDING_PER_SOURCE {
        return Err(QuarantineError::CapacityInvariant);
    }
    let local = usize::from(head)
        .checked_add(usize::from(length))
        .ok_or(QuarantineError::CapacityInvariant)?
        % MAX_PENDING_PER_SOURCE;
    source_index
        .checked_mul(MAX_PENDING_PER_SOURCE)
        .and_then(|base| base.checked_add(local))
        .ok_or(QuarantineError::CapacityInvariant)
}

/// Count a passing item, queue a refused item, or return a counted direct-manual event if its
/// source-specific queue is full. This function never applies accepted state changes.
pub fn route_input(
    queue: &mut QuarantineQueue,
    outcome: InputOutcome,
) -> Result<Disposition, QuarantineError> {
    let source = outcome.identity.source;
    let source_ix = source_index(source);
    let event = validated_event(outcome)?;
    let stats_before = *checked_ref(&queue.stats, source_ix)?;
    if let Some(event) = event {
        let mut next_stats = updated_reason_counts(stats_before, event.reasons.0)?;
        let head = *checked_ref(&queue.heads, source_ix)?;
        let length = *checked_ref(&queue.lengths, source_ix)?;
        if usize::from(length) == MAX_PENDING_PER_SOURCE {
            next_stats.queue_full_manual_events = add_one(next_stats.queue_full_manual_events)?;
            *checked_mut(&mut queue.stats, source_ix)? = next_stats;
            return Ok(Disposition::ManualHandlingRequired(event));
        }
        let index = event_index(source_ix, head, length)?;
        next_stats.queued = add_one(next_stats.queued)?;
        let next_length = length
            .checked_add(1)
            .ok_or(QuarantineError::CounterOverflow)?;
        let slot = checked_mut(&mut queue.events, index)?;
        let stored_length = checked_mut(&mut queue.lengths, source_ix)?;
        let stored_stats = checked_mut(&mut queue.stats, source_ix)?;
        *slot = event;
        *stored_length = next_length;
        *stored_stats = next_stats;
        Ok(Disposition::Queued(event))
    } else {
        let mut next_stats = stats_before;
        next_stats.accepted = add_one(next_stats.accepted)?;
        *checked_mut(&mut queue.stats, source_ix)? = next_stats;
        Ok(Disposition::Accepted)
    }
}

/// Read the next source-local event without removing it. The caller must explicitly acknowledge.
pub fn queue_front(
    queue: &QuarantineQueue,
    source: InputSource,
) -> Result<Option<QuarantineEvent>, QuarantineError> {
    let source_ix = source_index(source);
    let length = *checked_ref(&queue.lengths, source_ix)?;
    if length == 0 {
        return Ok(None);
    }
    let head = *checked_ref(&queue.heads, source_ix)?;
    let index = event_index(source_ix, head, 0)?;
    Ok(Some(*checked_ref(&queue.events, index)?))
}

/// Remove the previously presented front event after the caller's manual-handling workflow has
/// accepted responsibility for it. This is not an acknowledgement of safe operational recovery.
pub fn queue_acknowledge(
    queue: &mut QuarantineQueue,
    source: InputSource,
    receipt: oa_audit_core::AuditReceipt,
) -> Result<QuarantineEvent, QuarantineError> {
    let source_ix = source_index(source);
    let head = *checked_ref(&queue.heads, source_ix)?;
    let length = *checked_ref(&queue.lengths, source_ix)?;
    if length == 0 {
        return Err(QuarantineError::EmptyQueue);
    }
    let index = event_index(source_ix, head, 0)?;
    let event = *checked_ref(&queue.events, index)?;
    let (expected_target, expected_target_len) = release_audit_target(event);
    let next_audit_sequence = queue
        .release_audit_checkpoint
        .count
        .checked_add(1)
        .ok_or(QuarantineError::CounterOverflow)?;
    if oa_audit_core::validate_event(&receipt.event).is_err()
        || receipt.action() != oa_audit_core::Action::QuarantineRelease
        || !matches!(
            receipt.role(),
            oa_audit_core::Role::Operate | oa_audit_core::Role::Configure
        )
        || receipt.sequence() != next_audit_sequence
        || receipt.previous() != queue.release_audit_checkpoint.tail
        || receipt.target()
            != expected_target
                .get(..usize::from(expected_target_len))
                .unwrap_or(&[])
    {
        return Err(QuarantineError::AuditRequired);
    }
    let next_head = head
        .checked_add(1)
        .ok_or(QuarantineError::CounterOverflow)?
        % u16::try_from(MAX_PENDING_PER_SOURCE).map_err(|_| QuarantineError::CapacityInvariant)?;
    let next_length = length
        .checked_sub(1)
        .ok_or(QuarantineError::CapacityInvariant)?;
    let mut next_stats = *checked_ref(&queue.stats, source_ix)?;
    next_stats.acknowledged = add_one(next_stats.acknowledged)?;
    let stored_head = checked_mut(&mut queue.heads, source_ix)?;
    let stored_length = checked_mut(&mut queue.lengths, source_ix)?;
    let stored_stats = checked_mut(&mut queue.stats, source_ix)?;
    *stored_head = next_head;
    *stored_length = next_length;
    *stored_stats = next_stats;
    queue.release_audit_checkpoint = oa_audit_core::Checkpoint {
        count: receipt.sequence(),
        tail: receipt.digest(),
    };
    Ok(event)
}

/// Canonical target for the required pre-commit quarantine release audit record.
pub fn release_audit_target(event: QuarantineEvent) -> ([u8; oa_audit_core::TARGET_CAPACITY], u8) {
    let mut target = [0u8; oa_audit_core::TARGET_CAPACITY];
    let mut pos = 0usize;
    for byte in b"Q:" {
        if let Some(slot) = target.get_mut(pos) {
            *slot = *byte;
            pos += 1;
        }
    }
    put_hex(
        &mut target,
        &mut pos,
        u64::from(event.identity.source as u8),
        2,
    );
    if let Some(slot) = target.get_mut(pos) {
        *slot = b':';
        pos += 1;
    }
    put_hex(
        &mut target,
        &mut pos,
        u64::from(event.identity.source_instance),
        4,
    );
    if let Some(slot) = target.get_mut(pos) {
        *slot = b':';
        pos += 1;
    }
    put_hex(&mut target, &mut pos, event.identity.item_id, 16);
    (target, u8::try_from(pos).unwrap_or(0))
}

fn put_hex<const N: usize>(out: &mut [u8; N], pos: &mut usize, value: u64, digits: u32) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for digit in (0..digits).rev() {
        let nibble = usize::try_from((value >> (digit * 4)) & 0x0f).unwrap_or(0);
        if let (Some(slot), Some(byte)) = (out.get_mut(*pos), HEX.get(nibble)) {
            *slot = *byte;
            *pos += 1;
        }
    }
}

/// Return a copy of one source's counters.
pub fn source_stats(
    queue: &QuarantineQueue,
    source: InputSource,
) -> Result<SourceStats, QuarantineError> {
    Ok(*checked_ref(&queue.stats, source_index(source))?)
}

#[cfg(test)]
mod tests;

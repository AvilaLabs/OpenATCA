//! Independent, bounded check of externally paired source observations and track claims.
//!
//! This is a small offline core checker for NODE_NETWORK §§7 and 11, not a tracker, a matcher,
//! live wiring, or evidence of operational coverage. The caller must positively pair observations
//! and claims independently, supply the corresponding stable identity keys, and mark a pair's
//! shared local east/north frame as supported. This module compares those keys but does not prove
//! their provenance. Coordinates are signed millimetres in that caller-supplied frame; this module
//! makes no longitude, pole, or geodetic-frame claim. Every limit is caller-supplied, not a
//! production threshold.

/// Maximum paired rows examined by one call. Duplicate detection checks at most this many squared
/// row pairs; this cap makes that work bound explicit (RULESET C7).
pub const MAX_PAIRS: usize = 128;

/// `MonitorPair::presence`: a source observation is present.
pub const HAS_REFERENCE: u8 = 1;
/// `MonitorPair::presence`: a track claim is present.
pub const HAS_CLAIM: u8 = 2;
/// `MonitorPair::presence`: the caller verified both positions lie in the same supported,
/// trusted local frame.
pub const COMMON_LOCAL_FRAME: u8 = 4;
const PRESENCE_MASK: u8 = HAS_REFERENCE | HAS_CLAIM | COMMON_LOCAL_FRAME;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// One independently paired source observation and claimed track state. IDs are compared, never
/// matched by proximity; absent-side fields are ignored when their presence bit is clear.
// A3: this offline-only fixture keeps each bounded evidence pair adjacent; it is not a hot track
// collection, and the row bound is 128 pairs per call.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct MonitorPair {
    pub reference_id: u32,
    pub claim_id: u32,
    /// Source observation and track claim times, nanoseconds on the caller's shared time base.
    pub reference_time_ns: i64,
    pub claim_time_ns: i64,
    /// Local east/north position, millimetres, in the caller-verified common frame.
    pub reference_east_mm: i64,
    pub reference_north_mm: i64,
    pub claim_east_mm: i64,
    pub claim_north_mm: i64,
    /// Altitude in feet. The caller must provide the same altitude reference for both sides.
    pub reference_alt_ft: i32,
    pub claim_alt_ft: i32,
    /// [`HAS_REFERENCE`], [`HAS_CLAIM`] and [`COMMON_LOCAL_FRAME`]. Unknown bits are invalid.
    pub presence: u8,
}
// Budget: one plain input pair in the bounded independent monitor.
const _: () = assert!(core::mem::size_of::<MonitorPair>() == 72);

/// Bounds supplied by the caller for one qualification context.
// A7.2: cold caller configuration, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorLimits {
    /// Maximum age of either the reference or claimed track at `now_ns`.
    pub max_age_ns: u64,
    /// Maximum absolute time difference between the source observation and claim.
    pub max_pair_skew_ns: u64,
    /// Axis-aligned horizontal motion envelope, millimetres per second.
    pub max_horizontal_rate_mm_s: u32,
    /// Vertical motion envelope, feet per second.
    pub max_vertical_rate_ft_s: u32,
    /// Fixed horizontal allowance added to each east/north axis bound, millimetres.
    pub horizontal_tolerance_mm: u32,
    /// Fixed vertical allowance added to the altitude bound, feet.
    pub vertical_tolerance_ft: u32,
}

/// Result class for one supplied pair. `Contradiction` means the claim exceeded the supplied
/// envelope against the paired observation; it is not an independent ground-truth verdict. Only
/// `Healthy` means comparable and within that envelope; missing, stale, invalid and unsupported
/// evidence are never treated as healthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MonitorStatus {
    Healthy = 0,
    Contradiction = 1,
    MissingBoth = 2,
    MissingReference = 3,
    MissingClaim = 4,
    IdentityMismatch = 5,
    DuplicateEvidence = 6,
    FutureTimestamp = 7,
    StaleBoth = 8,
    StaleReference = 9,
    StaleClaim = 10,
    TimeSkew = 11,
    UnsupportedFrame = 12,
    UnsupportedRange = 13,
    InvalidEvidence = 14,
}

/// `MonitorFinding::violations`: east, north, altitude axes that exceeded their bounds.
pub const VIOLATION_EAST: u8 = 1;
pub const VIOLATION_NORTH: u8 = 2;
pub const VIOLATION_ALTITUDE: u8 = 4;

/// One result in input-pair order. The checker covers only supplied pairs; a caller represents an
/// unmatched source or claim by clearing the corresponding presence bit. It cannot discover rows
/// omitted entirely by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MonitorFinding {
    pub reference_id: u32,
    pub claim_id: u32,
    pub status: MonitorStatus,
    pub violations: u8,
}
// Budget: one finding per input pair.
const _: () = assert!(core::mem::size_of::<MonitorFinding>() == 12);

/// Counts for the supplied batch; every pair is either healthy, contradicted, or not comparable.
// A7.2: one-call summary, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorReport {
    pub pairs: u16,
    pub healthy: u16,
    pub contradicted: u16,
    pub not_comparable: u16,
}

/// A caller-owned output or input bound was insufficient. No findings are written on error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorError {
    TooManyPairs,
    FindingsTooShort,
}

fn axis_bound(rate_per_second: u32, elapsed_ns: u64, tolerance: u32) -> Option<u64> {
    u64::from(rate_per_second)
        .checked_mul(elapsed_ns)?
        .checked_div(NANOS_PER_SECOND)?
        .checked_add(u64::from(tolerance))
}

/// Repeated aircraft IDs at different times are expected; only identical ID/time keys conflict.
fn duplicate_evidence(pairs: &[MonitorPair], index: usize) -> bool {
    let pair = match pairs.get(index) {
        Some(pair) => pair,
        None => return false,
    };
    let has_reference = pair.presence & HAS_REFERENCE != 0;
    let has_claim = pair.presence & HAS_CLAIM != 0;
    if !has_reference && !has_claim {
        return false;
    }
    let mut other_index = 0;
    while other_index < pairs.len() {
        if other_index != index {
            if let Some(other) = pairs.get(other_index) {
                let duplicate_reference = has_reference
                    && other.presence & HAS_REFERENCE != 0
                    && other.reference_id == pair.reference_id
                    && other.reference_time_ns == pair.reference_time_ns;
                let duplicate_claim = has_claim
                    && other.presence & HAS_CLAIM != 0
                    && other.claim_id == pair.claim_id
                    && other.claim_time_ns == pair.claim_time_ns;
                if duplicate_reference || duplicate_claim {
                    return true;
                }
            }
        }
        other_index += 1;
    }
    false
}

fn check_pair(
    pair: &MonitorPair,
    pairs: &[MonitorPair],
    index: usize,
    now_ns: i64,
    limits: MonitorLimits,
) -> MonitorFinding {
    let mut finding = MonitorFinding {
        reference_id: pair.reference_id,
        claim_id: pair.claim_id,
        status: MonitorStatus::InvalidEvidence,
        violations: 0,
    };
    if pair.presence & !PRESENCE_MASK != 0 {
        return finding;
    }
    let has_reference = pair.presence & HAS_REFERENCE != 0;
    let has_claim = pair.presence & HAS_CLAIM != 0;
    match (has_reference, has_claim) {
        (false, false) => finding.status = MonitorStatus::MissingBoth,
        (false, true) => finding.status = MonitorStatus::MissingReference,
        (true, false) => finding.status = MonitorStatus::MissingClaim,
        (true, true) => {}
    }
    if !has_reference || !has_claim {
        return finding;
    }
    if duplicate_evidence(pairs, index) {
        finding.status = MonitorStatus::DuplicateEvidence;
        return finding;
    }
    if pair.reference_id != pair.claim_id {
        finding.status = MonitorStatus::IdentityMismatch;
        return finding;
    }
    if pair.reference_time_ns > now_ns || pair.claim_time_ns > now_ns {
        finding.status = MonitorStatus::FutureTimestamp;
        return finding;
    }
    let reference_age = now_ns.abs_diff(pair.reference_time_ns);
    let claim_age = now_ns.abs_diff(pair.claim_time_ns);
    let reference_stale = reference_age > limits.max_age_ns;
    let claim_stale = claim_age > limits.max_age_ns;
    match (reference_stale, claim_stale) {
        (true, true) => {
            finding.status = MonitorStatus::StaleBoth;
            return finding;
        }
        (true, false) => {
            finding.status = MonitorStatus::StaleReference;
            return finding;
        }
        (false, true) => {
            finding.status = MonitorStatus::StaleClaim;
            return finding;
        }
        (false, false) => {}
    }
    if pair.presence & COMMON_LOCAL_FRAME == 0 {
        finding.status = MonitorStatus::UnsupportedFrame;
        return finding;
    }
    let elapsed_ns = pair.reference_time_ns.abs_diff(pair.claim_time_ns);
    if elapsed_ns > limits.max_pair_skew_ns {
        finding.status = MonitorStatus::TimeSkew;
        return finding;
    }
    let Some(horizontal_bound) = axis_bound(
        limits.max_horizontal_rate_mm_s,
        elapsed_ns,
        limits.horizontal_tolerance_mm,
    ) else {
        finding.status = MonitorStatus::UnsupportedRange;
        return finding;
    };
    let Some(vertical_bound) = axis_bound(
        limits.max_vertical_rate_ft_s,
        elapsed_ns,
        limits.vertical_tolerance_ft,
    ) else {
        finding.status = MonitorStatus::UnsupportedRange;
        return finding;
    };
    if pair.reference_east_mm.abs_diff(pair.claim_east_mm) > horizontal_bound {
        finding.violations |= VIOLATION_EAST;
    }
    if pair.reference_north_mm.abs_diff(pair.claim_north_mm) > horizontal_bound {
        finding.violations |= VIOLATION_NORTH;
    }
    if pair.reference_alt_ft.abs_diff(pair.claim_alt_ft) > vertical_bound {
        finding.violations |= VIOLATION_ALTITUDE;
    }
    finding.status = if finding.violations == 0 {
        MonitorStatus::Healthy
    } else {
        MonitorStatus::Contradiction
    };
    finding
}

/// Checks already paired evidence using a caller-supplied axis-aligned physical envelope.
///
/// A row is comparable only when both sides are present, supplied identity keys agree, times are
/// plausible, the caller confirms a shared supported local plane, and checked integer bounds can
/// be computed. The motion
/// allowance is `rate * |claim_time - reference_time| + tolerance`, independently for east,
/// north and altitude. `now_ns` is explicit; any source or claim dated later than it is reported as
/// future evidence, including values near `i64::MAX`. No pair matching, geodesy, or threshold
/// selection is done here.
pub fn check_pairs(
    now_ns: i64,
    pairs: &[MonitorPair],
    limits: MonitorLimits,
    findings: &mut [MonitorFinding],
) -> Result<MonitorReport, MonitorError> {
    if pairs.len() > MAX_PAIRS {
        return Err(MonitorError::TooManyPairs);
    }
    if findings.len() < pairs.len() {
        return Err(MonitorError::FindingsTooShort);
    }
    let pair_count = match u16::try_from(pairs.len()) {
        Ok(n) => n,
        Err(_) => return Err(MonitorError::TooManyPairs),
    };
    let mut report = MonitorReport {
        pairs: pair_count,
        healthy: 0,
        contradicted: 0,
        not_comparable: 0,
    };
    let mut index = 0;
    while index < pairs.len() {
        if let Some(pair) = pairs.get(index) {
            let finding = check_pair(pair, pairs, index, now_ns, limits);
            if let Some(slot) = findings.get_mut(index) {
                *slot = finding;
            }
            match finding.status {
                MonitorStatus::Healthy => report.healthy += 1,
                MonitorStatus::Contradiction => report.contradicted += 1,
                _ => report.not_comparable += 1,
            }
        }
        index += 1;
    }
    Ok(report)
}

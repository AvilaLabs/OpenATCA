#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! Read-only Tech Ops evidence assessment (**core**, B1–B4, B7, B10).
//!
//! This crate compares a bounded, caller-supplied inventory and telemetry snapshot with explicit
//! expectations. It performs no I/O, reads no clock, authenticates no reporter, changes no service
//! state, and cannot run PVER stimuli or write a certification record. Matching report fields are
//! claims only: a successful comparison is always `ClaimsMatchUnverified`, never `PASS` and never
//! proof of authentication, approval, certification or deployment identity.
//!
//! Times are non-negative nanoseconds in an explicitly named monotonic clock domain. Every
//! inventory report is bound to one explicit `ServiceEpoch`; sequence coverage uses expected
//! `[sequence_first, sequence_next)` intervals supplied independently from reports, so missing
//! heads, interior slots and tails remain in the denominator. Stale, future, duplicate,
//! out-of-order, mismatched, unexpected and lossy rows remain counted and prevent complete status.
//! A supplied previous epoch is only an adjacent-claim comparison; without a trusted, loss-detecting
//! event history this cannot detect an A-to-B-to-A change or prove monotonicity across the run.
//! Callers must place all report and query times in the same explicitly identified monotonic clock
//! domain before using freshness comparisons; this core does not convert clocks.
//!
//! The fixed caps bound both storage and work: at most 128 expected processors, 65,536 inventory
//! rows, and 4,096 expected sequence slots per processor or performance window. At full inventory
//! and row caps, identity matching is at most 8,388,608 comparisons; the bitmap is 64 KiB, with
//! other state fixed. The 4,096-slot window covers over 68 minutes at one report per second; the
//! row cap admits 512 rows per expected processor on average, with all omissions still counted.
//! Excess input is refused before scans or scratch initialization. No collection allocates.

// RULESET B10: all core crates are built for 64-bit targets only.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("Tech Ops status core supports 64-bit targets only");

/// Bound for the controlled expected processor inventory (C2/C7).
pub const MAX_EXPECTED_PROCESSORS: usize = 128;
/// Bound for all report rows in one inventory snapshot (C2/C7).
pub const MAX_INVENTORY_REPORTS: usize = 65_536;
/// Maximum independently declared sequence slots per processor/window (C2/C7).
pub const MAX_SEQUENCE_SLOTS: u64 = 4_096;
/// Number of 64-bit words in one sequence bitmap.
pub const SEQUENCE_WORDS: usize = (MAX_SEQUENCE_SLOTS as usize) / 64;
/// Bound for performance samples in one metric window (C2/C7).
pub const MAX_PERFORMANCE_SAMPLES: usize = 65_536;

/// Stable processor identity from the expected inventory, not an authenticated identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ProcessorId(pub u32);

/// Producer/source identity from the expected inventory, not an authenticated identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct SourceId(pub u32);

/// Exact service configuration epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ServiceEpoch(pub u64);

/// Monotonic clock domain identity; timestamps from different domains are not comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ClockDomainId(pub u32);

/// Versioned profile identity supplied by the caller; the profile bytes are not authenticated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ProfileId(pub u32);

/// Versioned performance metric identity; its unit is fixed by the caller's approved profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct MetricId(pub u16);

/// Assessment class. No variant represents an operational PASS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AssessmentState {
    Incomplete = 1,
    Mismatch = 2,
    IncompleteAndMismatch = 3,
    /// All supplied claims compare and coverage is complete, but provenance is unverified.
    ClaimsMatchUnverified = 4,
    /// A complete self-reported window exceeds its caller-supplied limit; provenance is unverified.
    ClaimedLimitExceededUnverified = 5,
}

/// Why a bounded assessment was refused before it could produce a status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StatusError {
    EmptyInventory = 1,
    ExpectedInventoryOverCapacity = 2,
    ReportRowsOverCapacity = 3,
    DuplicateExpectedProcessor = 4,
    InvalidSequenceWindow = 5,
    InvalidQueryTime = 6,
    InvalidFreshnessLimit = 7,
    EmptyPerformanceWindow = 8,
    InvalidPerformanceWindow = 9,
    PerformanceSamplesOverCapacity = 10,
    CounterOverflow = 11,
    CapacityInvariant = 12,
}

/// Snapshot identities and time bounds shared by every inventory row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventoryQuery {
    pub previous_epoch: Option<ServiceEpoch>,
    pub epoch: ServiceEpoch,
    pub clock_domain_id: ClockDomainId,
    pub release_manifest_sha256: [u8; 32],
    pub query_time_ns: i64,
    pub freshness_limit_ns: i64,
}

/// Bit flags retained with every aggregate and processor record.
pub const FLAG_MISSING_PROCESSOR: u32 = 1 << 0;
pub const FLAG_MISSING_SEQUENCE: u32 = 1 << 1;
pub const FLAG_UNEXPECTED_PROCESSOR: u32 = 1 << 2;
pub const FLAG_DUPLICATE_SEQUENCE: u32 = 1 << 3;
pub const FLAG_SEQUENCE_OUT_OF_ORDER: u32 = 1 << 4;
pub const FLAG_TIME_OUT_OF_ORDER: u32 = 1 << 5;
pub const FLAG_STALE: u32 = 1 << 6;
pub const FLAG_FUTURE: u32 = 1 << 7;
pub const FLAG_SOURCE_MISMATCH: u32 = 1 << 8;
pub const FLAG_EPOCH_MISMATCH: u32 = 1 << 9;
pub const FLAG_CLOCK_DOMAIN_MISMATCH: u32 = 1 << 10;
pub const FLAG_RELEASE_MANIFEST_MISMATCH: u32 = 1 << 11;
pub const FLAG_FIRMWARE_MISMATCH: u32 = 1 << 12;
pub const FLAG_RUNNING_IMAGE_MISMATCH: u32 = 1 << 13;
pub const FLAG_LOADED_ADAPTATION_MISMATCH: u32 = 1 << 14;
pub const FLAG_DISK_ADAPTATION_MISMATCH: u32 = 1 << 15;
pub const FLAG_TELEMETRY_LOSS: u32 = 1 << 16;
pub const FLAG_SEQUENCE_OUTSIDE_WINDOW: u32 = 1 << 17;
pub const FLAG_PROFILE_MISMATCH: u32 = 1 << 18;
pub const FLAG_METRIC_MISMATCH: u32 = 1 << 19;
pub const FLAG_SAMPLE_OUTSIDE_WINDOW: u32 = 1 << 20;
pub const FLAG_INVALID_TIME: u32 = 1 << 21;
pub const FLAG_EPOCH_ORDER_UNKNOWN: u32 = 1 << 22;
pub const FLAG_EPOCH_REGRESSION: u32 = 1 << 23;

const INCOMPLETE_FLAGS: u32 = FLAG_MISSING_PROCESSOR
    | FLAG_MISSING_SEQUENCE
    | FLAG_UNEXPECTED_PROCESSOR
    | FLAG_DUPLICATE_SEQUENCE
    | FLAG_SEQUENCE_OUT_OF_ORDER
    | FLAG_TIME_OUT_OF_ORDER
    | FLAG_STALE
    | FLAG_FUTURE
    | FLAG_TELEMETRY_LOSS
    | FLAG_SEQUENCE_OUTSIDE_WINDOW
    | FLAG_SAMPLE_OUTSIDE_WINDOW
    | FLAG_INVALID_TIME
    | FLAG_EPOCH_ORDER_UNKNOWN;
const MISMATCH_FLAGS: u32 = FLAG_SOURCE_MISMATCH
    | FLAG_EPOCH_MISMATCH
    | FLAG_CLOCK_DOMAIN_MISMATCH
    | FLAG_RELEASE_MANIFEST_MISMATCH
    | FLAG_FIRMWARE_MISMATCH
    | FLAG_RUNNING_IMAGE_MISMATCH
    | FLAG_LOADED_ADAPTATION_MISMATCH
    | FLAG_DISK_ADAPTATION_MISMATCH
    | FLAG_PROFILE_MISMATCH
    | FLAG_METRIC_MISMATCH
    | FLAG_EPOCH_REGRESSION;

/// Expected inventory row; values come from a separately controlled manifest but are not
/// authenticated or independently verified by this pure comparator.
// A7.2: cold configuration record, one per expected processor per status snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ExpectedProcessor {
    pub processor_id: ProcessorId,
    pub source_id: SourceId,
    pub firmware_sha256: [u8; 32],
    pub running_image_sha256: [u8; 32],
    pub adaptation_sha256: [u8; 32],
    pub sequence_first: u64,
    /// Exclusive end of the independently declared expected sequence interval.
    pub sequence_next: u64,
}

/// One self-reported normalized processor sample. The digests are compared as byte claims only.
// A7.2: cold input record in a bounded offline status snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ProcessorReport {
    pub processor_id: ProcessorId,
    pub source_id: SourceId,
    pub epoch: ServiceEpoch,
    pub clock_domain_id: ClockDomainId,
    pub sequence: u64,
    pub sample_time_ns: i64,
    pub release_manifest_sha256: [u8; 32],
    pub firmware_sha256: [u8; 32],
    pub running_image_sha256: [u8; 32],
    pub loaded_adaptation_sha256: [u8; 32],
    pub disk_adaptation_sha256: [u8; 32],
    /// Number of source-reported lost events known at this sample.
    pub telemetry_loss_events: u32,
}

/// Per-processor counters and flags. Every expected processor has one row, including missing ones.
// A7.2: cold output record, at most one per expected processor per query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ProcessorAssessment {
    pub processor_id: ProcessorId,
    pub report_rows: u32,
    pub unique_sequences: u32,
    pub missing_sequences: u32,
    pub duplicate_sequences: u32,
    pub out_of_order_sequences: u32,
    pub out_of_order_times: u32,
    pub stale_rows: u32,
    pub future_rows: u32,
    pub invalid_time_rows: u32,
    pub source_mismatches: u32,
    pub epoch_mismatches: u32,
    pub clock_domain_mismatches: u32,
    pub release_manifest_mismatches: u32,
    pub firmware_mismatches: u32,
    pub running_image_mismatches: u32,
    pub loaded_adaptation_mismatches: u32,
    pub disk_adaptation_mismatches: u32,
    pub telemetry_loss_events: u64,
    pub sequences_outside_window: u32,
    pub flags: u32,
    /// Newest nonnegative, nonfuture sample time in the query clock domain; `-1` if absent.
    pub newest_sample_time_ns: i64,
}
const _: () = assert!(core::mem::size_of::<ProcessorAssessment>() == 96);

const EMPTY_PROCESSOR_ASSESSMENT: ProcessorAssessment = ProcessorAssessment {
    processor_id: ProcessorId(0),
    report_rows: 0,
    unique_sequences: 0,
    missing_sequences: 0,
    duplicate_sequences: 0,
    out_of_order_sequences: 0,
    out_of_order_times: 0,
    stale_rows: 0,
    future_rows: 0,
    invalid_time_rows: 0,
    source_mismatches: 0,
    epoch_mismatches: 0,
    clock_domain_mismatches: 0,
    release_manifest_mismatches: 0,
    firmware_mismatches: 0,
    running_image_mismatches: 0,
    loaded_adaptation_mismatches: 0,
    disk_adaptation_mismatches: 0,
    telemetry_loss_events: 0,
    sequences_outside_window: 0,
    flags: 0,
    newest_sample_time_ns: -1,
};

/// Complete bounded inventory assessment. `provenance_unverified` is always true in this crate.
// A7.2: one cold status reply per Tech Ops query; fixed-capacity rows avoid an allocator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InventoryAssessment {
    pub previous_epoch: Option<ServiceEpoch>,
    pub epoch: ServiceEpoch,
    pub epoch_order_checked: bool,
    pub epoch_regressed: bool,
    pub clock_domain_id: ClockDomainId,
    pub query_time_ns: i64,
    pub freshness_limit_ns: i64,
    pub release_manifest_sha256: [u8; 32],
    pub expected_processor_count: u32,
    pub report_rows: u32,
    pub unexpected_reports: u32,
    pub missing_processors: u32,
    pub expected_sequences: u64,
    pub unique_sequences: u64,
    pub missing_sequences: u64,
    pub duplicate_sequences: u32,
    pub out_of_order_sequences: u32,
    pub out_of_order_times: u32,
    pub stale_rows: u32,
    pub future_rows: u32,
    pub invalid_time_rows: u32,
    pub source_mismatches: u32,
    pub epoch_mismatches: u32,
    pub clock_domain_mismatches: u32,
    pub release_manifest_mismatches: u32,
    pub firmware_mismatches: u32,
    pub running_image_mismatches: u32,
    pub loaded_adaptation_mismatches: u32,
    pub disk_adaptation_mismatches: u32,
    pub telemetry_loss_events: u64,
    pub sequences_outside_window: u32,
    pub flags: u32,
    pub state: AssessmentState,
    /// Fixed by this API: content equality never authenticates a report.
    pub provenance_unverified: bool,
    pub processor_result_count: u32,
    pub processors: [ProcessorAssessment; MAX_EXPECTED_PROCESSORS],
}

/// An immutable-by-contract caller-supplied metric window expectation. Its profile and limits are
/// still caller claims; this evaluator has no profile store or approval authority.
// A7.2: cold configuration for one report window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MetricWindowContract {
    pub profile_id: ProfileId,
    pub metric_id: MetricId,
    pub processor_id: ProcessorId,
    pub previous_epoch: Option<ServiceEpoch>,
    pub epoch: ServiceEpoch,
    pub clock_domain_id: ClockDomainId,
    pub window_start_ns: i64,
    /// Exclusive end; closed windows must end no later than `query_time_ns`.
    pub window_end_ns: i64,
    pub sequence_first: u64,
    pub sequence_next: u64,
    /// Fixed metric units defined by `metric_id` in the caller's profile.
    pub maximum_allowed: i64,
}

/// One self-reported metric sample; the evaluator never infers omitted samples from this slice.
// A7.2: cold input record in a bounded offline metric window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PerformanceSample {
    pub profile_id: ProfileId,
    pub metric_id: MetricId,
    pub processor_id: ProcessorId,
    pub epoch: ServiceEpoch,
    pub clock_domain_id: ClockDomainId,
    pub sequence: u64,
    pub sample_time_ns: i64,
    pub value: i64,
    pub telemetry_loss_events: u32,
}

/// Assessment of one metric window, with both expected and observed denominators retained.
// A7.2: one cold metric result per Tech Ops query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MetricWindowAssessment {
    pub profile_id: ProfileId,
    pub metric_id: MetricId,
    pub processor_id: ProcessorId,
    pub previous_epoch: Option<ServiceEpoch>,
    pub epoch: ServiceEpoch,
    pub epoch_order_checked: bool,
    pub epoch_regressed: bool,
    pub clock_domain_id: ClockDomainId,
    pub query_time_ns: i64,
    pub freshness_limit_ns: i64,
    pub window_start_ns: i64,
    pub window_end_ns: i64,
    pub expected_samples: u32,
    pub reported_rows: u32,
    pub unique_sequences: u32,
    pub missing_samples: u32,
    pub duplicate_samples: u32,
    pub out_of_order_samples: u32,
    pub out_of_order_times: u32,
    pub stale_samples: u32,
    pub future_samples: u32,
    pub invalid_time_samples: u32,
    pub unexpected_samples: u32,
    pub sequences_outside_window: u32,
    pub profile_mismatches: u32,
    pub metric_mismatches: u32,
    pub epoch_mismatches: u32,
    pub clock_domain_mismatches: u32,
    pub sample_outside_window: u32,
    pub telemetry_loss_events: u64,
    pub usable_samples: u32,
    pub observed_maximum: i64,
    pub has_observed_maximum: bool,
    pub maximum_allowed: i64,
    pub maximum_exceeded: bool,
    pub flags: u32,
    pub state: AssessmentState,
    /// Fixed by this API: profile and sample provenance are never authenticated here.
    pub provenance_unverified: bool,
}

mod assess;
pub use assess::{assess_inventory, assess_metric_window};

mod clock;
pub use clock::{
    assess_daily_clock, gps_to_utc, parse_ubx_nav_timegps, utc_to_gps, ClockDailyResult,
    ClockError, ClockProcessorResult, ClockRowState, ClockSample, GpsInstant, ReferenceState,
    UbxGpsReference, UbxTimeGpsError, UtcInstant, GPS_EPOCH_UNIX_SECONDS, MAX_CLOCK_EXPECTED,
    MAX_CLOCK_SAMPLES,
};

mod monitor;
pub use monitor::{
    ack_alarm, configure_thresholds, event_intent, inventory_resource_inputs,
    record_alarm_transition, validate_latency_claim, view_monitor, Alarm, AlarmSeverity,
    AlarmState, AlarmTime, AlarmTransition, LatencyClaim, MonitorAction, MonitorError,
    MonitorEventIntent, MonitorRole, MonitorView, ResourceInput, ResourceKey, ResourceKind,
    ResourceOutput, ResourceState, ThresholdConfig, MAX_ALARMS, MAX_MONITOR_RESOURCES,
    MAX_TRACKS_PER_SCAN, REASON_INVENTORY_CLAIM_UNVERIFIED, REASON_INVENTORY_INCOMPLETE,
    REASON_INVENTORY_MISMATCH_OR_LOSS, REASON_INVENTORY_NO_REPORT,
    REASON_STALE_OR_INCOMPARABLE_CLOCK,
};

#[cfg(test)]
mod tests;

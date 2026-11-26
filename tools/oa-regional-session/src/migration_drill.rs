//! Bounded recorded-input preflight for the H5 flip/rollback drill.
//!
//! The OAF reader and normalizer remain authoritative for source decoding. This module measures
//! delivery of those normalized rows to a separate process and attempts M2 snapshot admission
//! without manufacturing missing system state. A successful input ACK is not an M2 flip ACK.
#![forbid(unsafe_code)]

use crate::input::{EventKind, FlightOperation, SourceKind};
use crate::recordings_cli::convert_recordings;
use crate::recordings_reader::{RecordingInput, RecordingSource};
use crate::recordings_stream::{
    source_coverage, stream_normalized_events, NormalizedStreamSummary, StreamCounts, StreamInput,
};
use crate::runtime::{regional_new_with_profile, regional_step, RegionalFrame, RegionalRunProfile};
use crate::session::{flush_receipt_group, push_event, session_cursor_new_with_limit};
use crate::surveillance::SurveillanceConfig;
use crate::ReceiptBatch;
use oa_airspace::core::model::{point_kind, str_at, Model};
use oa_migration::{
    empty_audit_log, evaluate_transition, new_unit, AuthorityApproval, AuthorityRole, Comparison,
    DependencyRequirement, EvidenceQuality, MetricCriterion, MetricEvidence, RollbackRule,
    RollbackTriggerPolicy, Stage, SwapUnitKey as M1SwapUnitKey, TransitionInput, TransitionPolicy,
    ROLE_ASSESSOR, ROLE_CONTROLLER_ON_POSITION, ROLE_SUPERVISOR, ROLE_TECH_OPS,
};
use oa_migration_switch::{
    authorize_window, ingest_view, new_promotion_intent, new_selector_with_capacity,
    observe_delivery, prepare_candidate_into, rollback, select_candidate, selected, AlertState,
    BusinessKey, CapacityProfile, Diff, EndpointClass, EntityFamily, EvidenceKind, FamilyCoverage,
    FieldStatus, FlightState, HandoffState, Health, Output, PromotionIntentRequest, RollbackKind,
    RollbackReceipt, Side, SnapshotView, SourceIdentity, SwapUnitKey, TrackState, UnitPolicy,
    REPLAY_ROWS_PER_FAMILY,
};
#[cfg(test)]
use oa_migration_switch::{Snapshot, MAX_ALERTS, MAX_FLIGHTS, MAX_HANDOFFS, MAX_TRACKS};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub const CYCLES: usize = 100;
pub const MAX_CONSUMER_FRAME_BYTES: usize = 2048;
/// H5 bounded materialized-vector cap. The full-day M4 source uses the profile cursor and a
/// streaming reader instead of retaining this many normalized rows.
pub const MAX_NORMALIZED_EVENTS: usize = 10_000_000;
pub const H5_CAPTURE_START_NS: i64 = 1_795_121_400_000_000_000;
pub const H5_MEASUREMENT_START_NS: i64 = 1_795_122_000_000_000_000;
pub const H5_MEASUREMENT_END_NS: i64 = 1_795_125_600_000_000_000;
/// Number of distinct addresses retained by the coarse spatial association filter.
pub const MAX_ZAU_BOX_ADDRESSES: usize = 4096;
/// Exact distinct-ever accounting uses a bounded ordered set (at most 250,000 u32 keys; allow
/// 16 MiB for tree nodes/allocator overhead) separately from concurrent membership.
pub const MAX_ZAU_DISTINCT_ADDRESSES: usize = 250_000;
/// Retained M2 projections are bounded to one 512 MiB logical payload per replay pass; each
/// pass keeps only the latest snapshot for each of 100 fixed bins. At 4096 rows per family this
/// admits typical sparse bins while refusing before the 6 GiB process budget can be consumed.
pub const MAX_H5_RETAINED_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024;
/// Only the current pair of same-time frame projections is retained during the one-pass replay.
/// This is separate from the 100-bin historical snapshot cap and refuses before consumer apply.
pub const MAX_H5_PAIRED_FRAME_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
/// Flight operations are spatially associated only while their last in-box position is this
/// recent. This is a filter freshness limit, not a claim about aircraft-state age.
pub const ZAU_POSITION_ASSOCIATION_MAX_AGE_NS: i64 = 60_000_000_000;
const MAX_WORKER_NOISE: usize = 4096;
const MAX_RUNTIME: Duration = Duration::from_secs(300);
const ACK_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_CLEANUP: Duration = Duration::from_secs(2);
const WORKER_PREFIX: &str = "H5CONSUMER\t1\t";

/// One bounded 4096-family replay state. The M2 selector copies these validated rows into its
/// startup-reserved profile; this shell-owned representation is retained only for the 100 bins.
#[derive(Clone, Debug, PartialEq)]
struct ReplaySnapshot {
    unit: SwapUnitKey,
    policy: UnitPolicy,
    evidence: EvidenceKind,
    side: Side,
    source_epoch: u64,
    state_version: u64,
    sequence: u64,
    source_time_ns: i64,
    observed_time_ns: i64,
    health: Health,
    track_coverage: FamilyCoverage,
    flight_coverage: FamilyCoverage,
    handoff_coverage: FamilyCoverage,
    alert_coverage: FamilyCoverage,
    tracks: Vec<Option<TrackState>>,
    flights: Vec<Option<FlightState>>,
    handoffs: Vec<Option<HandoffState>>,
    alerts: Vec<Option<AlertState>>,
    display_targets: Vec<oa_display::core::traffic::Target>,
}

impl ReplaySnapshot {
    fn view(&self) -> SnapshotView<'_> {
        SnapshotView {
            unit: self.unit,
            policy: self.policy,
            evidence: self.evidence,
            side: self.side,
            source_epoch: self.source_epoch,
            state_version: self.state_version,
            sequence: self.sequence,
            source_time_ns: self.source_time_ns,
            observed_time_ns: self.observed_time_ns,
            health: self.health,
            track_coverage: self.track_coverage,
            flight_coverage: self.flight_coverage,
            handoff_coverage: self.handoff_coverage,
            alert_coverage: self.alert_coverage,
            tracks: &self.tracks,
            flights: &self.flights,
            handoffs: &self.handoffs,
            alerts: &self.alerts,
        }
    }
}

fn replay_snapshot_bytes(snapshot: &ReplaySnapshot) -> Result<usize, String> {
    snapshot
        .tracks
        .len()
        .checked_mul(std::mem::size_of::<Option<TrackState>>())
        .and_then(|bytes| {
            snapshot
                .flights
                .len()
                .checked_mul(std::mem::size_of::<Option<FlightState>>())
                .and_then(|next| bytes.checked_add(next))
        })
        .and_then(|bytes| {
            snapshot
                .handoffs
                .len()
                .checked_mul(std::mem::size_of::<Option<HandoffState>>())
                .and_then(|next| bytes.checked_add(next))
        })
        .and_then(|bytes| {
            snapshot
                .alerts
                .len()
                .checked_mul(std::mem::size_of::<Option<AlertState>>())
                .and_then(|next| bytes.checked_add(next))
        })
        .and_then(|bytes| {
            snapshot
                .display_targets
                .len()
                .checked_mul(std::mem::size_of::<oa_display::core::traffic::Target>())
                .and_then(|next| bytes.checked_add(next))
        })
        .ok_or_else(|| "M2 replay snapshot byte arithmetic overflow".into())
}

fn store_replay_snapshot(
    snapshots: &mut [Option<ReplaySnapshot>],
    retained_bytes: &mut usize,
    index: usize,
    snapshot: ReplaySnapshot,
) -> Result<(), String> {
    let new_bytes = replay_snapshot_bytes(&snapshot)?;
    let slot = snapshots
        .get_mut(index)
        .ok_or("snapshot cycle index exceeds fixed bins")?;
    let previous_bytes = slot
        .as_ref()
        .map(replay_snapshot_bytes)
        .transpose()?
        .unwrap_or(0);
    let next_bytes = retained_bytes
        .checked_sub(previous_bytes)
        .and_then(|bytes| bytes.checked_add(new_bytes))
        .ok_or("M2 replay retained byte arithmetic overflow")?;
    if next_bytes > MAX_H5_RETAINED_SNAPSHOT_BYTES {
        return Err("M2 replay retained snapshot byte cap exceeded".into());
    }
    *slot = Some(snapshot);
    *retained_bytes = next_bytes;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerReceipt {
    pub sequence: u64,
    pub sha256: String,
    pub roundtrip_ns: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CycleAttempt {
    pub index: usize,
    pub start_ns: i64,
    pub end_ns: i64,
    pub position_inputs: u64,
    pub flight_inputs: u64,
    pub handoff_operations: u64,
    pub consumer_acks: u64,
    pub consumer_max_ack_ns: Option<u128>,
    pub replay_output_max_ack_ns: u128,
    pub replay_output_acks: u64,
    pub selected_state_applied_generation: Option<u64>,
    pub selected_state_applied_family_counts: Option<[u64; 4]>,
    pub selected_display_block_rows: Option<u64>,
    pub display_targets_equal: bool,
    pub selected_state_applied_sha256: Option<String>,
    pub decision_to_selected_output_ack_ns: Option<u128>,
    pub rollback_output_apply_ack_ns: Option<u128>,
    pub flip_rollback_attempted: bool,
    pub selected_output: &'static str,
    pub selector_export_dropped_baseline: [u64; 4],
    pub selector_export_dropped_candidate: [u64; 4],
    pub replay_projection_equal: bool,
    pub baseline_projection_sha256: String,
    pub second_projection_sha256: String,
    pub baseline_track_rows: u64,
    pub second_track_rows: u64,
    pub baseline_flight_views: u64,
    pub second_flight_views: u64,
    pub baseline_handoff_views: u64,
    pub second_handoff_views: u64,
    pub baseline_active_ca_alerts: u64,
    pub second_active_ca_alerts: u64,
    pub m2_ingest_refusal: Option<String>,
    pub m2_validation_ns: Option<u128>,
    pub m1_transition_refusal: Option<String>,
    pub m1_validation_ns: Option<u128>,
    pub flip_rollback_status: &'static str,
    pub unavailable: &'static str,
    /// Number of actual paired RegionalFrame updates applied to the local consumer in this bin.
    pub consumer_frame_updates: u64,
    /// Largest source-time difference between consecutive paired updates in this bin.
    pub consumer_max_virtual_gap_ns: Option<i64>,
    /// Largest measured monotonic ACK time for a selected-state update in this bin.
    pub consumer_max_apply_ack_ns: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerUpdateConsumerReport {
    pub updates: u64,
    pub virtual_gap_samples: u64,
    pub virtual_gap_limit_ns: i64,
    pub virtual_gap_exceeded: u64,
    pub max_virtual_gap_ns: Option<i64>,
    pub max_monotonic_apply_ack_ns: Option<u128>,
    pub decision_limit_ns: i64,
    pub decision_to_first_output_samples: u64,
    pub decision_to_first_output_exceeded: u64,
    pub max_decision_to_first_output_ns: Option<u128>,
    pub status: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordedDrillReport {
    pub run_profile: RegionalRunProfile,
    pub centre: String,
    pub endpoint_class: &'static str,
    pub update_interval_ns: i64,
    pub decision_limit_ns: i64,
    pub window_start_ns: i64,
    pub window_end_ns: i64,
    pub capture_start_ns: i64,
    pub warmup_event_count: u64,
    pub input_ack_count: u64,
    pub warmup_input_ack_count: u64,
    pub replay_output_ack_count: u64,
    pub input_streams: Vec<StreamReport>,
    pub stream_counts: StreamCounts,
    pub region_filter: ZauBoxFilterReport,
    pub normalized_event_count: u64,
    pub position_input_count: u64,
    pub flight_input_count: u64,
    pub handoff_operation_count: u64,
    pub quarantine_record_count: u64,
    pub manual_handling_required_count: u64,
    pub invalid_source_frame_count: u64,
    pub truncated_source_frame_count: u64,
    pub skipped_source_bytes: u64,
    pub source_files_refused: u64,
    pub consumer_receipts: Vec<ConsumerReceipt>,
    pub replay_consumer_receipts: Vec<ConsumerReceipt>,
    pub cycles: Vec<CycleAttempt>,
    pub per_update_consumer: PerUpdateConsumerReport,
    pub replica_cycles: [Vec<ReplicaCycle>; 2],
    pub model_sha256: String,
    pub map_centre_lat_e7: i32,
    pub map_centre_lon_e7: i32,
    pub map_range_nm: f64,
    pub source_state_families_available: [bool; 4],
    pub qualification: &'static str,
    pub authority_mode: &'static str,
    pub m2_attempt_count: u64,
    pub m2_refusal_count: u64,
    pub flip_rollback_attempt_count: u64,
    pub flip_rollback_complete_count: u64,
    pub max_retained_snapshot_bytes_per_replica: usize,
}

/// Coverage accounting for the explicit `oa_adsb::ZAU_BOX` rectangle. This is not an ARTCC
/// boundary polygon and does not imply complete coverage of ZAU traffic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZauBoxFilterReport {
    pub source: &'static str,
    pub lat_min_e7: i32,
    pub lat_max_e7: i32,
    pub lon_min_e7: i32,
    pub lon_max_e7: i32,
    pub position_rows_seen: u64,
    pub position_rows_in_box: u64,
    pub position_rows_outside_box: u64,
    pub position_rows_without_address: u64,
    pub active_in_box_addresses_at_end: u64,
    pub distinct_addresses_seen: u64,
    pub flight_rows_seen: u64,
    pub flight_rows_with_stale_in_box_position: u64,
    pub flight_rows_without_in_box_position: u64,
    pub expired_address_memberships: u64,
    pub other_rows_seen: u64,
    pub admitted_events: u64,
    pub address_cap: usize,
}

#[derive(Default)]
struct ZauBoxFilter {
    in_box_addresses: BTreeMap<u32, i64>,
    ever_in_box_addresses: BTreeSet<u32>,
    latest_receipt_ns: Option<i64>,
    report: ZauBoxFilterReportBuilder,
}

#[derive(Default)]
struct ZauBoxFilterReportBuilder {
    position_rows_seen: u64,
    position_rows_in_box: u64,
    position_rows_outside_box: u64,
    position_rows_without_address: u64,
    flight_rows_seen: u64,
    flight_rows_with_stale_in_box_position: u64,
    flight_rows_without_in_box_position: u64,
    expired_address_memberships: u64,
    other_rows_seen: u64,
    admitted_events: u64,
}

impl ZauBoxFilter {
    fn admit(&mut self, event: &crate::input::NormalizedEvent) -> Result<bool, String> {
        self.latest_receipt_ns = Some(
            self.latest_receipt_ns
                .map_or(event.receipt_ns, |latest| latest.max(event.receipt_ns)),
        );
        let box_filter = oa_adsb::ZAU_BOX;
        let admitted = match (&event.source, &event.kind) {
            (
                SourceKind::Surveillance,
                EventKind::Position {
                    latitude_e7,
                    longitude_e7,
                    ..
                },
            ) => {
                self.report.position_rows_seen = self
                    .report
                    .position_rows_seen
                    .checked_add(1)
                    .ok_or("ZAU box position count overflow")?;
                let inside = (*latitude_e7 >= box_filter.lat_min_e7)
                    && (*latitude_e7 <= box_filter.lat_max_e7)
                    && (*longitude_e7 >= box_filter.lon_min_e7)
                    && (*longitude_e7 <= box_filter.lon_max_e7);
                if inside {
                    self.report.position_rows_in_box = self
                        .report
                        .position_rows_in_box
                        .checked_add(1)
                        .ok_or("ZAU box in-box position count overflow")?;
                    if event.identity.aircraft_address == 0 {
                        self.report.position_rows_without_address = self
                            .report
                            .position_rows_without_address
                            .checked_add(1)
                            .ok_or("ZAU box unaddressed position count overflow")?;
                        false
                    } else {
                        if !self
                            .in_box_addresses
                            .contains_key(&event.identity.aircraft_address)
                            && self.in_box_addresses.len() >= MAX_ZAU_BOX_ADDRESSES
                        {
                            let before = self.in_box_addresses.len();
                            self.in_box_addresses.retain(|_, last_position_ns| {
                                i128::from(event.receipt_ns) - i128::from(*last_position_ns)
                                    <= i128::from(ZAU_POSITION_ASSOCIATION_MAX_AGE_NS)
                            });
                            let removed = before.saturating_sub(self.in_box_addresses.len());
                            self.report.expired_address_memberships = self
                                .report
                                .expired_address_memberships
                                .checked_add(
                                    u64::try_from(removed)
                                        .map_err(|_| "expired address count conversion overflow")?,
                                )
                                .ok_or("expired address membership count overflow")?;
                        }
                        if !self
                            .in_box_addresses
                            .contains_key(&event.identity.aircraft_address)
                            && self.in_box_addresses.len() >= MAX_ZAU_BOX_ADDRESSES
                        {
                            return Err("ZAU box distinct-address cap exceeded".into());
                        }
                        if !self
                            .ever_in_box_addresses
                            .contains(&event.identity.aircraft_address)
                            && self.ever_in_box_addresses.len() >= MAX_ZAU_DISTINCT_ADDRESSES
                        {
                            return Err("ZAU box total distinct-address cap exceeded".into());
                        }
                        self.in_box_addresses
                            .insert(event.identity.aircraft_address, event.receipt_ns);
                        self.ever_in_box_addresses
                            .insert(event.identity.aircraft_address);
                        true
                    }
                } else {
                    self.report.position_rows_outside_box = self
                        .report
                        .position_rows_outside_box
                        .checked_add(1)
                        .ok_or("ZAU box outside-position count overflow")?;
                    if event.identity.aircraft_address != 0 {
                        self.in_box_addresses
                            .remove(&event.identity.aircraft_address);
                    }
                    false
                }
            }
            (SourceKind::Flight, EventKind::Flight(_)) => {
                self.report.flight_rows_seen = self
                    .report
                    .flight_rows_seen
                    .checked_add(1)
                    .ok_or("ZAU box flight count overflow")?;
                let last_position = if event.identity.aircraft_address == 0 {
                    None
                } else {
                    self.in_box_addresses
                        .get(&event.identity.aircraft_address)
                        .copied()
                };
                if let Some(last_position_ns) = last_position {
                    let age_ns = i128::from(event.receipt_ns) - i128::from(last_position_ns);
                    if (0..=i128::from(ZAU_POSITION_ASSOCIATION_MAX_AGE_NS)).contains(&age_ns) {
                        true
                    } else {
                        self.report.flight_rows_with_stale_in_box_position = self
                            .report
                            .flight_rows_with_stale_in_box_position
                            .checked_add(1)
                            .ok_or("ZAU box stale-position flight count overflow")?;
                        if age_ns > i128::from(ZAU_POSITION_ASSOCIATION_MAX_AGE_NS) {
                            self.in_box_addresses
                                .remove(&event.identity.aircraft_address);
                            self.report.expired_address_memberships = self
                                .report
                                .expired_address_memberships
                                .checked_add(1)
                                .ok_or("expired address membership count overflow")?;
                        }
                        false
                    }
                } else {
                    self.report.flight_rows_without_in_box_position = self
                        .report
                        .flight_rows_without_in_box_position
                        .checked_add(1)
                        .ok_or("ZAU box unassociated flight count overflow")?;
                    false
                }
            }
            _ => {
                self.report.other_rows_seen = self
                    .report
                    .other_rows_seen
                    .checked_add(1)
                    .ok_or("ZAU box other-event count overflow")?;
                false
            }
        };
        if admitted {
            self.report.admitted_events = self
                .report
                .admitted_events
                .checked_add(1)
                .ok_or("ZAU box admitted-event count overflow")?;
        }
        Ok(admitted)
    }

    fn finish(mut self) -> Result<ZauBoxFilterReport, String> {
        if let Some(latest_receipt_ns) = self.latest_receipt_ns {
            let before = self.in_box_addresses.len();
            self.in_box_addresses.retain(|_, last_position_ns| {
                i128::from(latest_receipt_ns) - i128::from(*last_position_ns)
                    <= i128::from(ZAU_POSITION_ASSOCIATION_MAX_AGE_NS)
            });
            let removed = before.saturating_sub(self.in_box_addresses.len());
            self.report.expired_address_memberships = self
                .report
                .expired_address_memberships
                .checked_add(
                    u64::try_from(removed)
                        .map_err(|_| "expired address count conversion overflow")?,
                )
                .ok_or("expired address membership count overflow")?;
        }
        Ok(ZauBoxFilterReport {
            source: "oa_adsb::ZAU_BOX; coarse rectangle, not ARTCC boundary",
            lat_min_e7: oa_adsb::ZAU_BOX.lat_min_e7,
            lat_max_e7: oa_adsb::ZAU_BOX.lat_max_e7,
            lon_min_e7: oa_adsb::ZAU_BOX.lon_min_e7,
            lon_max_e7: oa_adsb::ZAU_BOX.lon_max_e7,
            position_rows_seen: self.report.position_rows_seen,
            position_rows_in_box: self.report.position_rows_in_box,
            position_rows_outside_box: self.report.position_rows_outside_box,
            position_rows_without_address: self.report.position_rows_without_address,
            active_in_box_addresses_at_end: u64::try_from(self.in_box_addresses.len())
                .map_err(|_| "ZAU box address count conversion overflow")?,
            distinct_addresses_seen: u64::try_from(self.ever_in_box_addresses.len())
                .map_err(|_| "ZAU box distinct address count conversion overflow")?,
            flight_rows_seen: self.report.flight_rows_seen,
            flight_rows_with_stale_in_box_position: self
                .report
                .flight_rows_with_stale_in_box_position,
            flight_rows_without_in_box_position: self.report.flight_rows_without_in_box_position,
            expired_address_memberships: self.report.expired_address_memberships,
            other_rows_seen: self.report.other_rows_seen,
            admitted_events: self.report.admitted_events,
            address_cap: MAX_ZAU_BOX_ADDRESSES,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamReport {
    pub source: &'static str,
    pub sha256: String,
    pub bytes: u64,
    pub messages: u64,
    pub first_frame_receive_ns: Option<i64>,
    pub last_frame_receive_ns: Option<i64>,
    pub max_frame_gap_ns: i64,
    pub source_full_window_covered: bool,
    pub source_largest_gap_ns: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaCycle {
    pub frames: u64,
    pub track_rows: u64,
    pub display_target_rows: u64,
    pub flight_views: u64,
    pub handoff_views: u64,
    pub active_ca_alerts: u64,
    pub ca_work_short: u64,
    pub ca_input_short: u64,
    pub ca_tracks_unscanned: u64,
    pub selector_export_dropped: [u64; 4],
    pub quarantined_items: u64,
    pub manual_handling_items: u64,
    pub last_frame_ns: Option<i64>,
    pub projection_sha256: String,
}

struct ReplicaCycleAccumulator {
    frames: u64,
    track_rows: u64,
    display_target_rows: u64,
    flight_views: u64,
    handoff_views: u64,
    active_ca_alerts: u64,
    ca_work_short: u64,
    ca_input_short: u64,
    ca_tracks_unscanned: u64,
    selector_export_dropped: [u64; 4],
    quarantined_items: u64,
    manual_handling_items: u64,
    last_frame_ns: Option<i64>,
    hash: oa_sha256::Sha256,
}

impl ReplicaCycleAccumulator {
    fn new() -> Self {
        Self {
            frames: 0,
            track_rows: 0,
            display_target_rows: 0,
            flight_views: 0,
            handoff_views: 0,
            active_ca_alerts: 0,
            ca_work_short: 0,
            ca_input_short: 0,
            ca_tracks_unscanned: 0,
            selector_export_dropped: [0; 4],
            quarantined_items: 0,
            manual_handling_items: 0,
            last_frame_ns: None,
            hash: oa_sha256::Sha256::new(),
        }
    }

    fn add_frame(&mut self, frame: &RegionalFrame) -> Result<(), String> {
        self.frames = self
            .frames
            .checked_add(1)
            .ok_or("replay frame count overflow")?;
        self.track_rows = self
            .track_rows
            .checked_add(
                u64::try_from(frame.surveillance.tracks.len())
                    .map_err(|_| "track count overflow")?,
            )
            .ok_or("track count overflow")?;
        self.display_target_rows = self
            .display_target_rows
            .checked_add(
                u64::try_from(frame.surveillance.display_targets.len())
                    .map_err(|_| "display target count overflow")?,
            )
            .ok_or("display target row count overflow")?;
        let flights = frame
            .flights
            .iter()
            .filter(|row| row.view.is_some())
            .count();
        let handoffs = frame
            .flights
            .iter()
            .filter(|row| {
                row.view
                    .is_some_and(|view| view.handoff_state != oa_flight::HO_NONE)
            })
            .count();
        self.selector_export_dropped = [
            u64::try_from(
                frame
                    .surveillance
                    .tracks
                    .len()
                    .saturating_sub(REPLAY_ROWS_PER_FAMILY),
            )
            .map_err(|_| "track export omission count overflow")?,
            u64::try_from(flights.saturating_sub(REPLAY_ROWS_PER_FAMILY))
                .map_err(|_| "flight export omission count overflow")?,
            u64::try_from(handoffs.saturating_sub(REPLAY_ROWS_PER_FAMILY))
                .map_err(|_| "handoff export omission count overflow")?,
            u64::try_from(
                frame
                    .surveillance
                    .ca_open
                    .len()
                    .saturating_sub(REPLAY_ROWS_PER_FAMILY),
            )
            .map_err(|_| "alert export omission count overflow")?,
        ];
        self.flight_views = self
            .flight_views
            .checked_add(u64::try_from(flights).map_err(|_| "flight view count overflow")?)
            .ok_or("flight view count overflow")?;
        // Count actual pending handoffs in this replica's own frozen flight views. A flight
        // snapshot is not itself a handoff; `HO_NONE` must remain outside this numerator.
        let pending_handoffs = frame
            .flights
            .iter()
            .filter(|row| {
                row.view
                    .is_some_and(|view| view.handoff_state != oa_flight::HO_NONE)
            })
            .count();
        self.handoff_views = self
            .handoff_views
            .checked_add(
                u64::try_from(pending_handoffs)
                    .map_err(|_| "pending handoff view count overflow")?,
            )
            .ok_or("handoff view count overflow")?;
        self.active_ca_alerts = self
            .active_ca_alerts
            .checked_add(
                u64::try_from(frame.surveillance.ca_open.len())
                    .map_err(|_| "alert count overflow")?,
            )
            .ok_or("alert count overflow")?;
        self.ca_work_short = self
            .ca_work_short
            .checked_add(frame.surveillance.ca_work_short)
            .ok_or("CA work-short count overflow")?;
        self.ca_input_short = self
            .ca_input_short
            .checked_add(frame.surveillance.ca_input_short)
            .ok_or("CA input-short count overflow")?;
        self.ca_tracks_unscanned = self
            .ca_tracks_unscanned
            .checked_add(frame.surveillance.ca_stats.tracks_unscanned)
            .ok_or("CA unscanned-track count overflow")?;
        self.quarantined_items = self
            .quarantined_items
            .checked_add(
                u64::try_from(frame.quarantine.records.len())
                    .map_err(|_| "quarantine count overflow")?,
            )
            .ok_or("quarantine count overflow")?;
        let manual = frame
            .quarantine
            .records
            .iter()
            .filter(|row| row.manual_handling_required)
            .count();
        self.manual_handling_items = self
            .manual_handling_items
            .checked_add(u64::try_from(manual).map_err(|_| "manual handling count overflow")?)
            .ok_or("manual handling count overflow")?;
        self.last_frame_ns = Some(frame.surveillance.time_ns);
        self.hash
            .update(&frame.surveillance.frame_sha256)
            .map_err(|_| "projection digest input too large")?;
        for row in &frame.surveillance.tracks {
            self.hash
                .update(&row.state_ns.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.newest_source_ns.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.address.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.lat_e7.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.lon_e7.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.altitude_ft.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.vertical_rate_fpm.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.east_mmps.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.north_mmps.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&row.r95_mm.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
        }
        for row in &frame.flights {
            if let Some(view) = row.view {
                self.hash
                    .update(&view.gufi.to_le_bytes())
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&view.address.to_le_bytes())
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&[view.handoff_state])
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&view.owner_unit)
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&view.handoff_from)
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&view.handoff_to)
                    .map_err(|_| "projection digest input too large")?;
                self.hash
                    .update(&view.handoff_accepted_by)
                    .map_err(|_| "projection digest input too large")?;
            }
        }
        for alert in &frame.surveillance.ca_open {
            self.hash
                .update(&alert.addr_a.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&alert.addr_b.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&alert.start_ns.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
            self.hash
                .update(&alert.end_ns.to_le_bytes())
                .map_err(|_| "projection digest input too large")?;
        }
        Ok(())
    }

    fn finish(self) -> ReplicaCycle {
        ReplicaCycle {
            frames: self.frames,
            track_rows: self.track_rows,
            display_target_rows: self.display_target_rows,
            flight_views: self.flight_views,
            handoff_views: self.handoff_views,
            active_ca_alerts: self.active_ca_alerts,
            ca_work_short: self.ca_work_short,
            ca_input_short: self.ca_input_short,
            ca_tracks_unscanned: self.ca_tracks_unscanned,
            selector_export_dropped: self.selector_export_dropped,
            quarantined_items: self.quarantined_items,
            manual_handling_items: self.manual_handling_items,
            last_frame_ns: self.last_frame_ns,
            projection_sha256: oa_sha256::to_hex(&self.hash.finalize()),
        }
    }
}

fn model_centre(model: &Model) -> Result<(i32, i32), String> {
    if model.meta.artcc != "ZAU" {
        return Err("model snapshot ARTCC is not ZAU".into());
    }
    let mut found = None;
    for index in 0..model.points.kind.len() {
        if model.points.kind.get(index) != Some(&point_kind::AIRPORT) {
            continue;
        }
        let ident = model
            .points
            .ident
            .get(index)
            .and_then(|ix| str_at(&model.strings, *ix));
        if ident == Some("KORD") {
            if found.is_some() {
                return Err("model has multiple KORD airport reference points".into());
            }
            let lat = *model
                .points
                .lat
                .get(index)
                .ok_or("KORD latitude column mismatch")?;
            let lon = *model
                .points
                .lon
                .get(index)
                .ok_or("KORD longitude column mismatch")?;
            if !(-900_000_000..=900_000_000).contains(&lat)
                || !(-1_800_000_000..=1_800_000_000).contains(&lon)
            {
                return Err("KORD model reference point is outside coordinate bounds".into());
            }
            found = Some((lat, lon));
        }
    }
    found.ok_or_else(|| "model has no KORD airport reference point".into())
}

fn apply_batch_observed(
    runtime: &mut crate::runtime::RegionalRuntime,
    batch: &ReceiptBatch,
    cycle: &mut ReplicaCycleAccumulator,
    cycle_index: usize,
    visit: &mut impl FnMut(usize, &RegionalFrame) -> Result<(), String>,
) -> Result<(), String> {
    let frame =
        regional_step(runtime, batch).map_err(|error| format!("regional replay: {error}"))?;
    cycle.add_frame(&frame)?;
    visit(cycle_index, &frame)
}

/// One bounded independent runtime replica for a streamed recorded replay. The caller creates
/// two instances and feeds each the same ordered normalized-event stream in separate reader
/// passes; this keeps the replay state independent without retaining a day of events or frames.
pub struct RegionalReplayAccumulator {
    runtime: crate::runtime::RegionalRuntime,
    cycles: Vec<ReplicaCycleAccumulator>,
    cursor: crate::session::SessionCursor,
    capture_start: i64,
    start: i64,
    end: i64,
}

/// Constructs one replay replica with its own decoder-independent runtime, session cursor, and
/// fixed 100-cycle aggregate table.
pub fn regional_replay_new(
    model_bytes: &[u8],
    model_sha: [u8; 32],
    range_nm: f64,
    start: i64,
    end: i64,
    profile: RegionalRunProfile,
) -> Result<RegionalReplayAccumulator, String> {
    regional_replay_new_with_warmup(model_bytes, model_sha, range_nm, start, start, end, profile)
}

/// Constructs a replay whose runtime ingests `[capture_start, measurement_start)` as warm-up.
/// Only frames in `[measurement_start, measurement_end)` enter the fixed 100-cycle aggregates
/// and visitor. The capture and measurement ranges are explicit, half-open intervals.
pub fn regional_replay_new_with_warmup(
    model_bytes: &[u8],
    model_sha: [u8; 32],
    range_nm: f64,
    capture_start: i64,
    measurement_start: i64,
    measurement_end: i64,
    profile: RegionalRunProfile,
) -> Result<RegionalReplayAccumulator, String> {
    if !range_nm.is_finite() || !(1.0..=1000.0).contains(&range_nm) {
        return Err("map range must be finite and in 1..=1000 NM".into());
    }
    if capture_start < 0
        || capture_start > measurement_start
        || measurement_start >= measurement_end
    {
        return Err("invalid replay time window".into());
    }
    let model = oa_airspace::core::snapshot::decode(model_bytes)
        .map_err(|error| format!("model snapshot decode: {error:?}"))?;
    let (centre_lat_e7, centre_lon_e7) = model_centre(&model)?;
    let runtime = regional_new_with_profile(
        model,
        model_sha,
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7,
            centre_lon_e7,
            range_nm,
        },
        profile,
    )?;
    let mut cycles = Vec::new();
    cycles
        .try_reserve_exact(CYCLES)
        .map_err(|_| "replay cycle summary allocation refused")?;
    cycles.extend((0..CYCLES).map(|_| ReplicaCycleAccumulator::new()));
    let cursor = session_cursor_new_with_limit(profile.max_total_events)
        .map_err(|error| format!("recorded profile session cursor: {error:?}"))?;
    Ok(RegionalReplayAccumulator {
        runtime,
        cycles,
        cursor,
        capture_start,
        start: measurement_start,
        end: measurement_end,
    })
}

/// Feeds one normalized event into this independent replay and immediately consumes any closed
/// receipt batch. The only retained source events are the cursor's bounded current receipt group.
pub fn regional_replay_push(
    replay: &mut RegionalReplayAccumulator,
    event: crate::input::NormalizedEvent,
) -> Result<(), String> {
    regional_replay_push_observed(replay, event, |_, _| Ok(()))
}

/// Admits one event and visits each actual runtime frame as its bounded receipt group closes.
/// A streaming source adapter can use this to project real state without retaining frame history.
pub fn regional_replay_push_observed(
    replay: &mut RegionalReplayAccumulator,
    event: crate::input::NormalizedEvent,
    mut visit: impl FnMut(usize, &RegionalFrame) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(batch) = push_event(&mut replay.cursor, event)
        .map_err(|error| format!("replay event ordering/admission: {error:?}"))?
    {
        apply_replay_batch(replay, &batch, &mut visit)?;
    }
    Ok(())
}

/// Flushes the final receipt group and returns aggregate results, dropping the runtime. No
/// normalized event, frame history, track identity, or alert-row history is returned.
pub fn regional_replay_finish(
    replay: RegionalReplayAccumulator,
) -> Result<Vec<ReplicaCycle>, String> {
    regional_replay_finish_observed(replay, |_, _| Ok(()))
}

/// Flushes the final receipt group and visits its actual runtime frame before returning the
/// bounded cycle aggregates.
pub fn regional_replay_finish_observed(
    mut replay: RegionalReplayAccumulator,
    mut visit: impl FnMut(usize, &RegionalFrame) -> Result<(), String>,
) -> Result<Vec<ReplicaCycle>, String> {
    if let Some(batch) = flush_receipt_group(&mut replay.cursor)
        .map_err(|error| format!("replay final receipt group: {error:?}"))?
    {
        apply_replay_batch(&mut replay, &batch, &mut visit)?;
    }
    Ok(replay
        .cycles
        .into_iter()
        .map(ReplicaCycleAccumulator::finish)
        .collect())
}

fn apply_replay_batch(
    replay: &mut RegionalReplayAccumulator,
    batch: &ReceiptBatch,
    visit: &mut impl FnMut(usize, &RegionalFrame) -> Result<(), String>,
) -> Result<(), String> {
    let measurement_cycle = replay_measurement_cycle(
        batch.receipt_ns,
        replay.capture_start,
        replay.start,
        replay.end,
    )?;
    let Some(index) = measurement_cycle else {
        // Warm-up updates the same actual runtime; it is excluded from measurement counters and
        // visitor outputs, not filtered out of the replay state.
        regional_step(&mut replay.runtime, batch)
            .map_err(|error| format!("regional warm-up: {error}"))?;
        return Ok(());
    };
    apply_batch_observed(
        &mut replay.runtime,
        batch,
        replay.cycles.get_mut(index).ok_or("cycle index")?,
        index,
        visit,
    )
}

fn replay_measurement_cycle(
    receipt_ns: i64,
    capture_start: i64,
    measurement_start: i64,
    measurement_end: i64,
) -> Result<Option<usize>, String> {
    if capture_start < 0
        || capture_start > measurement_start
        || measurement_start >= measurement_end
        || receipt_ns < capture_start
        || receipt_ns >= measurement_end
    {
        return Err("replay receipt lies outside the declared capture interval".into());
    }
    if receipt_ns < measurement_start {
        return Ok(None);
    }
    row_cycle_in_span(receipt_ns, measurement_start, measurement_end).map(Some)
}

struct ReplayRun {
    cycles: Vec<ReplicaCycle>,
    snapshots: Vec<Option<ReplaySnapshot>>,
    retained_snapshot_bytes: usize,
}

#[derive(Clone, Copy)]
struct ReplaySpec<'a> {
    model_bytes: &'a [u8],
    model_sha: [u8; 32],
    range_nm: f64,
    capture_start: i64,
    start: i64,
    end: i64,
    profile: RegionalRunProfile,
    snapshot_policy: UnitPolicy,
    side: Side,
    source_epoch: u64,
    local_authority_fixture: bool,
}

fn cycle_window(index: usize, start: i64, end: i64) -> Result<SwapUnitKey, String> {
    if index >= CYCLES || start < 0 || start >= end {
        return Err("invalid replay cycle window".into());
    }
    let span = i128::from(end)
        .checked_sub(i128::from(start))
        .ok_or("replay time span overflow")?;
    let lo = i128::from(start) + span * index as i128 / CYCLES as i128;
    let hi = i128::from(start) + span * (index + 1) as i128 / CYCLES as i128;
    let window_start_ns = i64::try_from(lo).map_err(|_| "cycle start overflow")?;
    let window_end_ns = i64::try_from(hi).map_err(|_| "cycle end overflow")?;
    if window_end_ns <= window_start_ns {
        return Err("recorded time window is shorter than cycle count".into());
    }
    Ok(SwapUnitKey {
        function_id: 1,
        scope_id: 1,
        window_start_ns,
        window_end_ns,
    })
}

fn replay_replica(
    events: &[crate::input::NormalizedEvent],
    spec: ReplaySpec<'_>,
) -> Result<ReplayRun, String> {
    let mut replay = regional_replay_new_with_warmup(
        spec.model_bytes,
        spec.model_sha,
        spec.range_nm,
        spec.capture_start,
        spec.start,
        spec.end,
        spec.profile,
    )?;
    let mut snapshots = vec![None; CYCLES];
    let mut retained_snapshot_bytes = 0usize;
    for event in events {
        regional_replay_push_observed(&mut replay, *event, |index, frame| {
            let unit = cycle_window(index, spec.start, spec.end)?;
            let snapshot = snapshot_from_runtime_frame(
                frame,
                unit,
                spec.snapshot_policy,
                spec.side,
                spec.source_epoch,
                spec.local_authority_fixture,
            )?;
            store_replay_snapshot(
                &mut snapshots,
                &mut retained_snapshot_bytes,
                index,
                snapshot,
            )
        })?;
    }
    let cycles = regional_replay_finish_observed(replay, |index, frame| {
        let unit = cycle_window(index, spec.start, spec.end)?;
        let snapshot = snapshot_from_runtime_frame(
            frame,
            unit,
            spec.snapshot_policy,
            spec.side,
            spec.source_epoch,
            spec.local_authority_fixture,
        )?;
        store_replay_snapshot(
            &mut snapshots,
            &mut retained_snapshot_bytes,
            index,
            snapshot,
        )
    })?;
    Ok(ReplayRun {
        cycles,
        snapshots,
        retained_snapshot_bytes,
    })
}

struct StreamPassCounts {
    normalized_events: u64,
    positions: u64,
    flights: u64,
    handoffs: u64,
    warmup_events: u64,
    input_ack_count: u64,
    warmup_ack_count: u64,
    cycle_input_acks: [u64; CYCLES],
    cycle_input_max_ack_ns: [u128; CYCLES],
    cycle_positions: [u64; CYCLES],
    cycle_flights: [u64; CYCLES],
    cycle_handoffs: [u64; CYCLES],
}

struct StreamReplaySpec<'a> {
    inputs: &'a [StreamInput],
    centre: [u8; 4],
    replay: ReplaySpec<'a>,
    run_started: Instant,
}

fn stream_replay_pass(
    spec: StreamReplaySpec<'_>,
    mut input_consumer: Option<&mut Consumer>,
) -> Result<
    (
        ReplayRun,
        NormalizedStreamSummary,
        StreamPassCounts,
        ZauBoxFilterReport,
    ),
    String,
> {
    let mut replay = regional_replay_new_with_warmup(
        spec.replay.model_bytes,
        spec.replay.model_sha,
        spec.replay.range_nm,
        spec.replay.capture_start,
        spec.replay.start,
        spec.replay.end,
        spec.replay.profile,
    )?;
    let mut snapshots = vec![None; CYCLES];
    let mut retained_snapshot_bytes = 0usize;
    let mut counts = StreamPassCounts {
        normalized_events: 0,
        positions: 0,
        flights: 0,
        handoffs: 0,
        warmup_events: 0,
        input_ack_count: 0,
        warmup_ack_count: 0,
        cycle_input_acks: [0; CYCLES],
        cycle_input_max_ack_ns: [0; CYCLES],
        cycle_positions: [0; CYCLES],
        cycle_flights: [0; CYCLES],
        cycle_handoffs: [0; CYCLES],
    };
    let mut consumer_sequence = 1u64;
    let mut region_filter = ZauBoxFilter::default();
    let summary = stream_normalized_events(
        spec.inputs,
        Some((spec.replay.capture_start, spec.replay.end)),
        spec.centre,
        |event, _provenance| {
            if spec.run_started.elapsed() > MAX_RUNTIME {
                return Err("recorded H5 runtime cap exceeded".into());
            }
            counts.normalized_events = counts
                .normalized_events
                .checked_add(1)
                .ok_or("normalized event count overflow")?;
            if counts.normalized_events > spec.replay.profile.max_total_events as u64 {
                return Err("recorded profile normalized event cap exceeded".into());
            }
            if !region_filter.admit(&event)? {
                return Ok(());
            }
            if event.receipt_ns < H5_MEASUREMENT_START_NS {
                counts.warmup_events = counts
                    .warmup_events
                    .checked_add(1)
                    .ok_or("warm-up event count overflow")?;
            } else {
                let index = row_cycle_in_span(
                    event.receipt_ns,
                    H5_MEASUREMENT_START_NS,
                    H5_MEASUREMENT_END_NS,
                )?;
                match (&event.source, &event.kind) {
                    (SourceKind::Surveillance, EventKind::Position { .. }) => {
                        let value = counts
                            .cycle_positions
                            .get_mut(index)
                            .ok_or("position cycle index")?;
                        *value = value
                            .checked_add(1)
                            .ok_or("cycle position count overflow")?;
                    }
                    (SourceKind::Flight, EventKind::Flight(_)) => {
                        let value = counts
                            .cycle_flights
                            .get_mut(index)
                            .ok_or("flight cycle index")?;
                        *value = value.checked_add(1).ok_or("cycle flight count overflow")?;
                        if handoff_event(&event) {
                            let value = counts
                                .cycle_handoffs
                                .get_mut(index)
                                .ok_or("handoff cycle index")?;
                            *value = value.checked_add(1).ok_or("cycle handoff count overflow")?;
                        }
                    }
                    _ => (),
                }
            }
            match (&event.source, &event.kind) {
                (SourceKind::Surveillance, EventKind::Position { .. }) => {
                    counts.positions = counts
                        .positions
                        .checked_add(1)
                        .ok_or("position event count overflow")?;
                }
                (SourceKind::Flight, EventKind::Flight(_)) => {
                    counts.flights = counts
                        .flights
                        .checked_add(1)
                        .ok_or("flight event count overflow")?;
                    if handoff_event(&event) {
                        counts.handoffs = counts
                            .handoffs
                            .checked_add(1)
                            .ok_or("handoff event count overflow")?;
                    }
                }
                _ => (),
            }
            if let Some(consumer) = input_consumer.as_deref_mut() {
                let serialized =
                    crate::recordings::serialize_recorded_events(std::slice::from_ref(&event))
                        .map_err(|error| {
                            format!("normalize consumer row serialization: {error:?}")
                        })?;
                let payload = serialized
                    .split_once('\n')
                    .map(|(_, body)| body.trim_end_matches('\n'))
                    .ok_or("normalized event serialization missing header")?;
                let receipt = consumer.deliver(consumer_sequence, payload.as_bytes())?;
                consumer_sequence = consumer_sequence
                    .checked_add(1)
                    .ok_or("consumer sequence overflow")?;
                counts.input_ack_count = counts
                    .input_ack_count
                    .checked_add(1)
                    .ok_or("input ACK count overflow")?;
                if event.receipt_ns < H5_MEASUREMENT_START_NS {
                    counts.warmup_ack_count = counts
                        .warmup_ack_count
                        .checked_add(1)
                        .ok_or("warm-up ACK count overflow")?;
                } else {
                    let index = row_cycle_in_span(
                        event.receipt_ns,
                        H5_MEASUREMENT_START_NS,
                        H5_MEASUREMENT_END_NS,
                    )?;
                    let ack_count = counts
                        .cycle_input_acks
                        .get_mut(index)
                        .ok_or("input ACK cycle index")?;
                    *ack_count = ack_count
                        .checked_add(1)
                        .ok_or("input cycle ACK count overflow")?;
                    let max_ack = counts
                        .cycle_input_max_ack_ns
                        .get_mut(index)
                        .ok_or("input ACK max cycle index")?;
                    *max_ack = (*max_ack).max(receipt.roundtrip_ns);
                }
            }
            regional_replay_push_observed(&mut replay, event, |index, frame| {
                let unit = cycle_window(index, spec.replay.start, spec.replay.end)?;
                let snapshot = snapshot_from_runtime_frame(
                    frame,
                    unit,
                    spec.replay.snapshot_policy,
                    spec.replay.side,
                    spec.replay.source_epoch,
                    spec.replay.local_authority_fixture,
                )?;
                store_replay_snapshot(
                    &mut snapshots,
                    &mut retained_snapshot_bytes,
                    index,
                    snapshot,
                )
            })
        },
    )?;
    if summary.normalized_events != counts.normalized_events {
        return Err("normalized stream summary/event callback count mismatch".into());
    }
    let cycles = regional_replay_finish_observed(replay, |index, frame| {
        let unit = cycle_window(index, spec.replay.start, spec.replay.end)?;
        let snapshot = snapshot_from_runtime_frame(
            frame,
            unit,
            spec.replay.snapshot_policy,
            spec.replay.side,
            spec.replay.source_epoch,
            spec.replay.local_authority_fixture,
        )?;
        store_replay_snapshot(
            &mut snapshots,
            &mut retained_snapshot_bytes,
            index,
            snapshot,
        )
    })?;
    if spec.replay.local_authority_fixture {
        let source_activity = source_coverage(
            &summary.recording.streams,
            RecordingSource::Adsb,
            (spec.replay.capture_start, spec.replay.end),
            spec.replay.snapshot_policy.update_interval_ns,
        )?;
        let local_source_healthy = source_activity.full_window_covered
            && summary.normalized_events > 0
            && summary.recording.counts.frames > 0
            && summary.recording.counts.reject_frames == 0
            && summary.quarantine_rows == 0
            && summary.manual_handling_required == 0
            && summary.unprocessed_frames == 0;
        if !local_source_healthy {
            for snapshot in snapshots.iter_mut().flatten() {
                snapshot.health = Health::Unknown;
                snapshot.track_coverage = FamilyCoverage::Partial;
                snapshot.flight_coverage = FamilyCoverage::Partial;
                snapshot.handoff_coverage = FamilyCoverage::Partial;
                snapshot.alert_coverage = FamilyCoverage::Partial;
            }
        }
    }
    Ok((
        ReplayRun {
            cycles,
            snapshots,
            retained_snapshot_bytes,
        },
        summary,
        counts,
        region_filter.finish()?,
    ))
}

struct SelectedStateBatch {
    rows: Vec<Vec<u8>>,
    family_counts: [u64; 4],
    sha256: String,
}

fn hex_encode(bytes: &[u8]) -> Result<String, String> {
    let mut out = String::new();
    out.try_reserve_exact(bytes.len().saturating_mul(2))
        .map_err(|_| "state hex allocation refused")?;
    for byte in bytes {
        use std::fmt::Write as FmtWrite;
        write!(&mut out, "{byte:02x}").map_err(|_| "state hex formatting failed")?;
    }
    Ok(out)
}

fn update_state_batch_digest(hash: &mut oa_sha256::Sha256, payload: &[u8]) -> Result<(), String> {
    let len = u32::try_from(payload.len()).map_err(|_| "state row length conversion overflow")?;
    hash.update(&len.to_le_bytes())
        .map_err(|_| "state batch digest input too large")?;
    hash.update(payload)
        .map_err(|_| "state batch digest input too large".to_string())
}

fn state_batch_sha256(payloads: &[Vec<u8>]) -> Result<String, String> {
    let mut hash = oa_sha256::Sha256::new();
    for payload in payloads {
        update_state_batch_digest(&mut hash, payload)?;
    }
    Ok(oa_sha256::to_hex(&hash.finalize()))
}

fn snapshot_output_payloads(
    snapshot: SnapshotView<'_>,
    display_targets: &[oa_display::core::traffic::Target],
    selected_side: Output,
) -> Result<SelectedStateBatch, String> {
    let count = snapshot
        .tracks
        .iter()
        .flatten()
        .count()
        .checked_add(snapshot.flights.iter().flatten().count())
        .and_then(|value| value.checked_add(snapshot.handoffs.iter().flatten().count()))
        .and_then(|value| value.checked_add(snapshot.alerts.iter().flatten().count()))
        .ok_or("selected output row count overflow")?;
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(count)
        .map_err(|_| "selected output row allocation refused")?;
    let selected_side = match selected_side {
        Output::Legacy => "Legacy",
        Output::Candidate => "Candidate",
    };
    let mut family_counts = [0u64; 4];
    for row in snapshot.tracks.iter().flatten() {
        family_counts[0] = family_counts[0]
            .checked_add(1)
            .ok_or("selected track count overflow")?;
        let address =
            u32::try_from(row.key.ordinal).map_err(|_| "track display join address overflow")?;
        let display = display_targets
            .binary_search_by_key(&address, |target| target.addr)
            .ok()
            .and_then(|index| display_targets.get(index))
            .copied()
            .map(AppliedDisplayTarget::from);
        if display.as_ref().is_some_and(|target| {
            target.addr != address
                || target.lat_e7 != row.lat_e7
                || target.lon_e7 != row.lon_e7
                || target.alt_ft != row.altitude_ft
        }) {
            return Err("display target does not match its selected track row".into());
        }
        let display = serialize_display_target(display.as_ref())?;
        payloads.push(format!(
            "H5STATE\t1\tselected={selected_side}\tTRACK\tkey={}\tordinal={}\tstatus={}\tlat_e7={}\tlon_e7={}\taltitude_ft={}\tdigest={}\t{}",
            hex_encode(&row.key.entity_key)?, row.key.ordinal, row.status as u8,
            row.lat_e7, row.lon_e7, row.altitude_ft, hex_encode(&row.projection_digest)?, display,
        ).into_bytes());
    }
    for row in snapshot.flights.iter().flatten() {
        family_counts[1] = family_counts[1]
            .checked_add(1)
            .ok_or("selected flight count overflow")?;
        payloads.push(
            format!(
                "H5STATE\t1\tselected={selected_side}\tFLIGHT\tkey={}\tordinal={}\tstatus={}\tplan_digest={}\tdigest={}",
                hex_encode(&row.key.entity_key)?, row.key.ordinal, row.status as u8,
                hex_encode(&row.flight_plan_digest)?, hex_encode(&row.projection_digest)?,
            )
            .into_bytes(),
        );
    }
    for row in snapshot.handoffs.iter().flatten() {
        family_counts[2] = family_counts[2]
            .checked_add(1)
            .ok_or("selected handoff count overflow")?;
        payloads.push(
            format!(
                "H5STATE\t1\tselected={selected_side}\tHANDOFF\tkey={}\tordinal={}\tstatus={}\tlifecycle={}\towner={}\trecipient={}\tdigest={}",
                hex_encode(&row.key.entity_key)?, row.key.ordinal, row.status as u8,
                row.lifecycle, row.owner, row.recipient, hex_encode(&row.projection_digest)?,
            )
            .into_bytes(),
        );
    }
    for row in snapshot.alerts.iter().flatten() {
        family_counts[3] = family_counts[3]
            .checked_add(1)
            .ok_or("selected alert count overflow")?;
        payloads.push(
            format!(
                "H5STATE\t1\tselected={selected_side}\tALERT\tkey={}\tordinal={}\tstatus={}\tactive={}\tack=UNKNOWN\tsuppressed=UNKNOWN\tdigest={}",
                hex_encode(&row.key.entity_key)?, row.key.ordinal, row.status as u8, row.active,
                hex_encode(&row.projection_digest)?,
            )
            .into_bytes(),
        );
    }
    if payloads
        .iter()
        .any(|row| row.len() > MAX_CONSUMER_FRAME_BYTES / 2 - 128)
    {
        return Err("selected output row exceeds consumer payload cap".into());
    }
    let sha256 = state_batch_sha256(&payloads)?;
    Ok(SelectedStateBatch {
        rows: payloads,
        family_counts,
        sha256,
    })
}

fn h5_stream_reports(
    summary: &NormalizedStreamSummary,
    start_ns: i64,
    end_ns: i64,
    allowed_gap_ns: i64,
) -> Result<Vec<StreamReport>, String> {
    let mut reports = Vec::new();
    reports
        .try_reserve_exact(summary.recording.streams.len())
        .map_err(|_| "H5 stream report allocation refused")?;
    for source in [RecordingSource::Adsb, RecordingSource::Sfdps] {
        let coverage = source_coverage(
            &summary.recording.streams,
            source,
            (start_ns, end_ns),
            allowed_gap_ns,
        )?;
        for stream in summary
            .recording
            .streams
            .iter()
            .filter(|stream| stream.source == source)
        {
            reports.push(StreamReport {
                source: match source {
                    RecordingSource::Adsb => "ADSB",
                    RecordingSource::Sfdps => "SFDPS",
                },
                sha256: oa_sha256::to_hex(&stream.sha256),
                bytes: stream.bytes,
                messages: stream.messages,
                first_frame_receive_ns: stream.first_frame_receive_ns,
                last_frame_receive_ns: stream.last_frame_receive_ns,
                max_frame_gap_ns: stream.max_frame_gap_ns,
                source_full_window_covered: coverage.full_window_covered,
                source_largest_gap_ns: coverage.largest_gap_ns,
            });
        }
    }
    Ok(reports)
}

fn parse_worker_sequence(line: &[u8], expected: u64, expected_hash: &str) -> Result<(), String> {
    let text = std::str::from_utf8(line).map_err(|_| "consumer response is not UTF-8")?;
    let fields = text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
    if fields.len() != 5
        || fields.first().copied() != Some("H5CONSUMER")
        || fields.get(1).copied() != Some("1")
        || fields.get(2).copied() != Some("ACK")
        || fields.get(3).copied() != Some(expected.to_string().as_str())
        || fields.get(4).copied() != Some(expected_hash)
    {
        return Err("consumer ACK does not bind to delivered sequence and payload".into());
    }
    Ok(())
}

fn worker_read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::with_capacity(128);
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte)? {
            0 if frame.is_empty() => return Ok(None),
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial frame",
                ));
            }
            _ if byte[0] == b'\n' => {
                frame.push(b'\n');
                return Ok(Some(frame));
            }
            _ => {
                if frame.len() >= MAX_CONSUMER_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "frame cap exceeded",
                    ));
                }
                frame.push(byte[0]);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AppliedStateRow {
    Track {
        key: [u8; 32],
        ordinal: u64,
        status: u8,
        lat_e7: i32,
        lon_e7: i32,
        altitude_ft: i32,
        display: Option<AppliedDisplayTarget>,
        digest: [u8; 32],
    },
    Flight {
        key: [u8; 32],
        ordinal: u64,
        status: u8,
        plan_digest: [u8; 32],
        digest: [u8; 32],
    },
    Handoff {
        key: [u8; 32],
        ordinal: u64,
        status: u8,
        lifecycle: u8,
        owner: u32,
        recipient: u32,
        digest: [u8; 32],
    },
    Alert {
        key: [u8; 32],
        ordinal: u64,
        status: u8,
        active: bool,
        ack_unknown: bool,
        suppressed_unknown: bool,
        digest: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedDisplayTarget {
    lat_e7: i32,
    lon_e7: i32,
    alt_ft: i32,
    addr: u32,
    r95_bits: u32,
    callsign: [u8; 8],
    track_ddeg: u16,
    gs_dkt: u16,
    reacq_age_s: u16,
    flags: u8,
    trail_len: u8,
    trail: [(i32, i32); oa_display::core::traffic::MAX_TRAIL],
}

impl From<oa_display::core::traffic::Target> for AppliedDisplayTarget {
    fn from(target: oa_display::core::traffic::Target) -> Self {
        Self {
            lat_e7: target.lat_e7,
            lon_e7: target.lon_e7,
            alt_ft: target.alt_ft,
            addr: target.addr,
            r95_bits: target.r95_m.to_bits(),
            callsign: target.callsign,
            track_ddeg: target.track_ddeg,
            gs_dkt: target.gs_dkt,
            reacq_age_s: target.reacq_age_s,
            flags: target.flags,
            trail_len: target.trail_len,
            trail: target.trail,
        }
    }
}

impl AppliedStateRow {
    fn key_ordinal(&self) -> (&[u8; 32], u64) {
        match self {
            Self::Track { key, ordinal, .. }
            | Self::Flight { key, ordinal, .. }
            | Self::Handoff { key, ordinal, .. }
            | Self::Alert { key, ordinal, .. } => (key, *ordinal),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedState {
    generation: u64,
    selected: &'static str,
    tables: [Vec<AppliedStateRow>; 4],
    digest: String,
}

impl Default for AppliedState {
    fn default() -> Self {
        Self {
            generation: 0,
            selected: "NONE",
            tables: std::array::from_fn(|_| Vec::new()),
            digest: String::new(),
        }
    }
}

struct PendingAppliedState {
    cycle: u64,
    generation: u64,
    selected: &'static str,
    expected: [usize; 4],
    tables: [Vec<AppliedStateRow>; 4],
    digest: oa_sha256::Sha256,
    next_sequence: u64,
}

impl PendingAppliedState {
    fn new(
        cycle: u64,
        generation: u64,
        selected: &'static str,
        expected: [usize; 4],
    ) -> Result<Self, String> {
        let caps = [REPLAY_ROWS_PER_FAMILY; 4];
        if expected.iter().zip(caps).any(|(count, cap)| *count > cap) {
            return Err("selected state family cap exceeded".into());
        }
        let mut tables: [Vec<AppliedStateRow>; 4] = std::array::from_fn(|_| Vec::new());
        for (table, count) in tables.iter_mut().zip(expected) {
            table
                .try_reserve_exact(count)
                .map_err(|_| "selected state allocation refused")?;
        }
        Ok(Self {
            cycle,
            generation,
            selected,
            expected,
            tables,
            digest: oa_sha256::Sha256::new(),
            next_sequence: 1,
        })
    }

    fn push(&mut self, sequence: u64, declared_sha: &str, payload: &[u8]) -> Result<(), String> {
        if sequence != self.next_sequence || payload.len() > MAX_CONSUMER_FRAME_BYTES / 2 - 128 {
            return Err("selected state row sequence or size is invalid".into());
        }
        let actual_sha = oa_sha256::to_hex(&oa_sha256::sha256(payload));
        if actual_sha != declared_sha {
            return Err("selected state row SHA-256 mismatch".into());
        }
        let (row, family) = parse_applied_state_row(payload, self.selected)?;
        let canonical = serialize_applied_state_row(&row, self.selected)?;
        if canonical.as_slice() != payload {
            return Err("selected state row is not the canonical typed serialization".into());
        }
        let table = self.tables.get_mut(family).ok_or("state family index")?;
        if table.len()
            >= *self
                .expected
                .get(family)
                .ok_or("state family count index")?
        {
            return Err("selected state row exceeds declared family count".into());
        }
        update_state_batch_digest(&mut self.digest, &canonical)?;
        table.push(row);
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or("selected state sequence overflow")?;
        Ok(())
    }

    fn apply(
        mut self,
        current: &mut AppliedState,
        cycle: u64,
        generation: u64,
        counts: [usize; 4],
        declared_digest: &str,
    ) -> Result<String, String> {
        if cycle != self.cycle
            || generation != self.generation
            || generation
                != current
                    .generation
                    .checked_add(1)
                    .ok_or("state generation overflow")?
            || counts != self.expected
            || self.tables.each_ref().map(|table| table.len()) != self.expected
        {
            return Err("selected state cycle generation or row counts mismatch".into());
        }
        for table in &mut self.tables {
            table.sort_unstable_by(|left, right| left.key_ordinal().cmp(&right.key_ordinal()));
            if table.windows(2).any(|rows| {
                rows.first().map(|row| row.key_ordinal())
                    == rows.get(1).map(|row| row.key_ordinal())
            }) {
                return Err("selected state contains duplicate family business key".into());
            }
        }
        let actual_digest = oa_sha256::to_hex(&self.digest.finalize());
        if actual_digest != declared_digest {
            return Err("selected state aggregate digest mismatch".into());
        }
        current.generation = generation;
        current.selected = self.selected;
        current.tables = self.tables;
        current.digest = actual_digest.clone();
        Ok(actual_digest)
    }
}

fn field_value<'a>(field: &'a str, name: &str) -> Result<&'a str, String> {
    field
        .strip_prefix(name)
        .and_then(|value| value.strip_prefix('='))
        .ok_or_else(|| format!("selected state field {name} is missing or malformed"))
}

fn required_field<'a>(fields: &'a [&'a str], index: usize, label: &str) -> Result<&'a str, String> {
    fields
        .get(index)
        .copied()
        .ok_or_else(|| format!("selected state {label} field is missing"))
}

fn parse_hex_array<const N: usize>(value: &str) -> Result<[u8; N], String> {
    if value.len() != N.saturating_mul(2) {
        return Err("selected state hex field has the wrong length".into());
    }
    let mut out = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let high = digit(*pair.first().ok_or("hex pair incomplete")?)
            .ok_or("selected state hex field is invalid")?;
        let low = digit(*pair.get(1).ok_or("hex pair incomplete")?)
            .ok_or("selected state hex field is invalid")?;
        *out.get_mut(index).ok_or("selected state hex index")? = (high << 4) | low;
    }
    Ok(out)
}

fn serialize_display_target(target: Option<&AppliedDisplayTarget>) -> Result<String, String> {
    let Some(target) = target else {
        return Ok(String::from("display=NONE"));
    };
    if usize::from(target.trail_len) > oa_display::core::traffic::MAX_TRAIL {
        return Err("regional display trail length is invalid".into());
    }
    let mut trail = String::new();
    for (index, (lat, lon)) in target.trail.iter().enumerate() {
        if index != 0 {
            trail.push(';');
        }
        use std::fmt::Write as FmtWrite;
        write!(&mut trail, "{lat}:{lon}").map_err(|_| "display trail formatting failed")?;
    }
    Ok(format!(
        "display={},{},{},{},{},{},{},{},{},{},{},{}",
        target.lat_e7,
        target.lon_e7,
        target.alt_ft,
        target.addr,
        target.r95_bits,
        hex_encode(&target.callsign)?,
        target.track_ddeg,
        target.gs_dkt,
        target.reacq_age_s,
        target.flags,
        target.trail_len,
        trail
    ))
}

fn display_targets_equal(
    left: &[oa_display::core::traffic::Target],
    right: &[oa_display::core::traffic::Target],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.lat_e7 == right.lat_e7
                && left.lon_e7 == right.lon_e7
                && left.alt_ft == right.alt_ft
                && left.addr == right.addr
                && left.r95_m.to_bits() == right.r95_m.to_bits()
                && left.trail == right.trail
                && left.callsign == right.callsign
                && left.track_ddeg == right.track_ddeg
                && left.gs_dkt == right.gs_dkt
                && left.reacq_age_s == right.reacq_age_s
                && left.flags == right.flags
                && left.trail_len == right.trail_len
        })
}

fn parse_display_target(value: &str) -> Result<Option<AppliedDisplayTarget>, String> {
    if value == "NONE" {
        return Ok(None);
    }
    let fields = value.split(',').collect::<Vec<_>>();
    if fields.len() != 12 {
        return Err("regional display target field count is invalid".into());
    }
    let number = |index: usize, name: &str| -> Result<u64, String> {
        fields
            .get(index)
            .copied()
            .ok_or("regional display target field missing")?
            .parse::<u64>()
            .map_err(|_| format!("regional display target {name} is invalid"))
    };
    let signed = |index: usize, name: &str| -> Result<i32, String> {
        fields
            .get(index)
            .copied()
            .ok_or("regional display target field missing")?
            .parse::<i32>()
            .map_err(|_| format!("regional display target {name} is invalid"))
    };
    let trail_text = fields.get(11).copied().ok_or("display trail missing")?;
    let mut trail = [(0, 0); oa_display::core::traffic::MAX_TRAIL];
    let trail_rows = trail_text.split(';').collect::<Vec<_>>();
    if trail_rows.len() != oa_display::core::traffic::MAX_TRAIL {
        return Err("regional display trail row count is invalid".into());
    }
    for (index, row) in trail_rows.iter().enumerate() {
        let (lat, lon) = row
            .split_once(':')
            .ok_or("regional display trail pair is malformed")?;
        *trail
            .get_mut(index)
            .ok_or("regional display trail index exceeds capacity")? = (
            lat.parse()
                .map_err(|_| "regional display trail latitude is invalid")?,
            lon.parse()
                .map_err(|_| "regional display trail longitude is invalid")?,
        );
    }
    let display = AppliedDisplayTarget {
        lat_e7: signed(0, "latitude")?,
        lon_e7: signed(1, "longitude")?,
        alt_ft: signed(2, "altitude")?,
        addr: u32::try_from(number(3, "address")?)
            .map_err(|_| "regional display address is out of range")?,
        r95_bits: u32::try_from(number(4, "r95 bits")?)
            .map_err(|_| "regional display radius bits are out of range")?,
        callsign: parse_hex_array(fields.get(5).copied().ok_or("display callsign missing")?)?,
        track_ddeg: u16::try_from(number(6, "track")?)
            .map_err(|_| "regional display track is out of range")?,
        gs_dkt: u16::try_from(number(7, "ground speed")?)
            .map_err(|_| "regional display speed is out of range")?,
        reacq_age_s: u16::try_from(number(8, "reacquisition age")?)
            .map_err(|_| "regional display reacquisition age is out of range")?,
        flags: u8::try_from(number(9, "flags")?)
            .map_err(|_| "regional display flags are out of range")?,
        trail_len: u8::try_from(number(10, "trail length")?)
            .map_err(|_| "regional display trail length is out of range")?,
        trail,
    };
    if usize::from(display.trail_len) > oa_display::core::traffic::MAX_TRAIL {
        return Err("regional display trail length exceeds the fixed trail capacity".into());
    }
    if !oa_display::core::traffic::position_valid(display.lat_e7, display.lon_e7)
        || display
            .trail
            .iter()
            .take(usize::from(display.trail_len))
            .any(|point| !oa_display::core::traffic::position_valid(point.0, point.1))
        || display.flags
            & !(oa_display::core::traffic::T_COAST
                | oa_display::core::traffic::T_UNCERTAIN
                | oa_display::core::traffic::T_CS_CONFLICT)
            != 0
    {
        return Err("regional display target fails the existing builder validity rules".into());
    }
    Ok(Some(display))
}

fn parse_applied_state_row(
    payload: &[u8],
    selected: &'static str,
) -> Result<(AppliedStateRow, usize), String> {
    let text = std::str::from_utf8(payload).map_err(|_| "selected state row is not UTF-8")?;
    let fields = text.split('\t').collect::<Vec<_>>();
    let side = if selected == "Legacy" {
        "selected=Legacy"
    } else {
        "selected=Candidate"
    };
    if fields.first() != Some(&"H5STATE")
        || fields.get(1) != Some(&"1")
        || fields.get(2).copied() != Some(side)
    {
        return Err("selected state row header or generation side is invalid".into());
    }
    let kind = fields
        .get(3)
        .copied()
        .ok_or("selected state family is missing")?;
    let number = |index: usize, name: &str| -> Result<u64, String> {
        field_value(
            fields
                .get(index)
                .copied()
                .ok_or("state numeric field missing")?,
            name,
        )?
        .parse::<u64>()
        .map_err(|_| format!("selected state {name} is invalid"))
    };
    let status = |index: usize| -> Result<u8, String> {
        let value = u8::try_from(number(index, "status")?)
            .map_err(|_| "selected state status is out of range")?;
        if value > FieldStatus::Missing as u8 {
            return Err("selected state status is unknown".into());
        }
        Ok(value)
    };
    let key = |index: usize| -> Result<[u8; 32], String> {
        parse_hex_array(field_value(
            fields.get(index).copied().ok_or("state key missing")?,
            "key",
        )?)
    };
    let digest = |index: usize, name: &str| -> Result<[u8; 32], String> {
        parse_hex_array(field_value(
            fields.get(index).copied().ok_or("state digest missing")?,
            name,
        )?)
    };
    let row = match kind {
        "TRACK" if fields.len() == 12 => {
            let ordinal = number(5, "ordinal")?;
            let lat_e7 = field_value(required_field(&fields, 7, "latitude")?, "lat_e7")?
                .parse()
                .map_err(|_| "selected track latitude is invalid")?;
            let lon_e7 = field_value(required_field(&fields, 8, "longitude")?, "lon_e7")?
                .parse()
                .map_err(|_| "selected track longitude is invalid")?;
            let altitude_ft = field_value(required_field(&fields, 9, "altitude")?, "altitude_ft")?
                .parse()
                .map_err(|_| "selected track altitude is invalid")?;
            let display = parse_display_target(field_value(
                fields
                    .get(11)
                    .copied()
                    .ok_or("display target field missing")?,
                "display",
            )?)?;
            if display.as_ref().is_some_and(|target| {
                u64::from(target.addr) != ordinal
                    || target.lat_e7 != lat_e7
                    || target.lon_e7 != lon_e7
                    || target.alt_ft != altitude_ft
            }) {
                return Err(
                    "selected display block identity/position differs from track row".into(),
                );
            }
            (
                AppliedStateRow::Track {
                    key: key(4)?,
                    ordinal,
                    status: status(6)?,
                    lat_e7,
                    lon_e7,
                    altitude_ft,
                    display,
                    digest: digest(10, "digest")?,
                },
                0,
            )
        }
        "FLIGHT" if fields.len() == 9 => (
            AppliedStateRow::Flight {
                key: key(4)?,
                ordinal: number(5, "ordinal")?,
                status: status(6)?,
                plan_digest: digest(7, "plan_digest")?,
                digest: digest(8, "digest")?,
            },
            1,
        ),
        "HANDOFF" if fields.len() == 11 => {
            let lifecycle = u8::try_from(number(7, "lifecycle")?)
                .map_err(|_| "selected handoff lifecycle is out of range")?;
            if ![oa_flight::HO_OFFERED, oa_flight::HO_ACCEPTED].contains(&lifecycle) {
                return Err("selected handoff lifecycle is invalid".into());
            }
            (
                AppliedStateRow::Handoff {
                    key: key(4)?,
                    ordinal: number(5, "ordinal")?,
                    status: status(6)?,
                    lifecycle,
                    owner: u32::try_from(number(8, "owner")?)
                        .map_err(|_| "selected handoff owner is out of range")?,
                    recipient: u32::try_from(number(9, "recipient")?)
                        .map_err(|_| "selected handoff recipient is out of range")?,
                    digest: digest(10, "digest")?,
                },
                2,
            )
        }
        "ALERT" if fields.len() == 11 => {
            let active = match field_value(required_field(&fields, 7, "active")?, "active")? {
                "0" => false,
                "1" => true,
                _ => return Err("selected alert active flag is invalid".into()),
            };
            if required_field(&fields, 8, "acknowledgement")? != "ack=UNKNOWN"
                || required_field(&fields, 9, "suppression")? != "suppressed=UNKNOWN"
            {
                return Err(
                    "selected alert acknowledgement/suppression must remain unknown".into(),
                );
            }
            (
                AppliedStateRow::Alert {
                    key: key(4)?,
                    ordinal: number(5, "ordinal")?,
                    status: status(6)?,
                    active,
                    ack_unknown: true,
                    suppressed_unknown: true,
                    digest: digest(10, "digest")?,
                },
                3,
            )
        }
        _ => return Err("selected state family or field count is invalid".into()),
    };
    Ok(row)
}

fn serialize_applied_state_row(row: &AppliedStateRow, selected: &str) -> Result<Vec<u8>, String> {
    let bytes = match row {
        AppliedStateRow::Track {
            key,
            ordinal,
            status,
            lat_e7,
            lon_e7,
            altitude_ft,
            display,
            digest,
        } => format!(
            "H5STATE\t1\tselected={selected}\tTRACK\tkey={}\tordinal={ordinal}\tstatus={status}\tlat_e7={lat_e7}\tlon_e7={lon_e7}\taltitude_ft={altitude_ft}\tdigest={}\t{}",
            hex_encode(key)?,
            hex_encode(digest)?,
            serialize_display_target(display.as_ref())?,
        ),
        AppliedStateRow::Flight {
            key,
            ordinal,
            status,
            plan_digest,
            digest,
        } => format!(
            "H5STATE\t1\tselected={selected}\tFLIGHT\tkey={}\tordinal={ordinal}\tstatus={status}\tplan_digest={}\tdigest={}",
            hex_encode(key)?,
            hex_encode(plan_digest)?,
            hex_encode(digest)?,
        ),
        AppliedStateRow::Handoff {
            key,
            ordinal,
            status,
            lifecycle,
            owner,
            recipient,
            digest,
        } => format!(
            "H5STATE\t1\tselected={selected}\tHANDOFF\tkey={}\tordinal={ordinal}\tstatus={status}\tlifecycle={lifecycle}\towner={owner}\trecipient={recipient}\tdigest={}",
            hex_encode(key)?,
            hex_encode(digest)?,
        ),
        AppliedStateRow::Alert {
            key,
            ordinal,
            status,
            active,
            ack_unknown,
            suppressed_unknown,
            digest,
        } => {
            if !ack_unknown || !suppressed_unknown {
                return Err(
                    "selected alert acknowledgement/suppression must remain unknown".into(),
                );
            }
            format!(
                "H5STATE\t1\tselected={selected}\tALERT\tkey={}\tordinal={ordinal}\tstatus={status}\tactive={}\tack=UNKNOWN\tsuppressed=UNKNOWN\tdigest={}",
                hex_encode(key)?,
                u8::from(*active),
                hex_encode(digest)?,
            )
        }
    };
    Ok(bytes.into_bytes())
}

fn parse_state_counts(fields: &[&str], offset: usize) -> Result<[usize; 4], String> {
    let mut counts = [0usize; 4];
    for (index, (slot, name)) in counts
        .iter_mut()
        .zip(["tracks", "flights", "handoffs", "alerts"])
        .enumerate()
    {
        *slot = field_value(
            fields
                .get(offset + index)
                .copied()
                .ok_or("state count missing")?,
            name,
        )?
        .parse::<usize>()
        .map_err(|_| "selected state family count is invalid")?;
    }
    Ok(counts)
}

fn decode_lower_hex_payload(payload_hex: &str) -> Result<Vec<u8>, String> {
    if payload_hex.len() % 2 != 0 || payload_hex.len() / 2 > MAX_CONSUMER_FRAME_BYTES / 2 - 128 {
        return Err("consumer payload hex size is invalid".into());
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(payload_hex.len() / 2)
        .map_err(|_| "consumer payload allocation refused")?;
    for pair in payload_hex.as_bytes().chunks_exact(2) {
        let digit = |value: u8| match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        };
        let high = digit(*pair.first().ok_or("payload hex pair is incomplete")?)
            .ok_or("payload hex is invalid")?;
        let low = digit(*pair.get(1).ok_or("payload hex pair is incomplete")?)
            .ok_or("payload hex is invalid")?;
        payload.push((high << 4) | low);
    }
    Ok(payload)
}

pub fn consumer_worker() -> Result<(), String> {
    // seal-allow: consume only this child's bounded protocol stdin pipe, never a path or recording.
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let mut expected = 1u64;
    let mut total = 0usize;
    let mut cycle_messages = 0u64;
    let mut applied = AppliedState::default();
    let mut pending: Option<PendingAppliedState> = None;
    while let Some(line) = worker_read_frame(&mut input).map_err(|error| error.to_string())? {
        total = total
            .checked_add(line.len())
            .ok_or("worker input count overflow")?;
        if total > MAX_NORMALIZED_EVENTS.saturating_mul(MAX_CONSUMER_FRAME_BYTES) {
            return Err("worker total input cap exceeded".into());
        }
        let text = std::str::from_utf8(&line).map_err(|_| "request is not UTF-8")?;
        let mut fields = text.trim_end_matches('\n').split('\t');
        if fields.next() != Some("H5CONSUMER") || fields.next() != Some("1") {
            return Err("unknown consumer command".into());
        }
        let command = fields.next().ok_or("missing consumer command")?;
        if command == "END_CYCLE" {
            if pending.is_some() {
                return Err("cannot end transport cycle during state application".into());
            }
            let cycle = fields.next().ok_or("missing cycle index")?;
            if fields.next().is_some() {
                return Err("cycle boundary shape is invalid".into());
            }
            writeln!(
                output,
                "H5CONSUMER\t1\tCYCLE_ACK\t{cycle}\t{cycle_messages}"
            )
            .map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
            expected = 1;
            cycle_messages = 0;
            continue;
        }
        if command == "STATE_BEGIN" {
            let rest = fields.collect::<Vec<_>>();
            if rest.len() != 7 || pending.is_some() || cycle_messages != 0 || expected != 1 {
                return Err(
                    "selected state begin shape, nesting, or transport boundary is invalid".into(),
                );
            }
            let cycle = rest
                .first()
                .copied()
                .ok_or("state cycle is missing")?
                .parse::<u64>()
                .map_err(|_| "selected state cycle is invalid")?;
            let generation = rest
                .get(1)
                .copied()
                .ok_or("state generation is missing")?
                .parse::<u64>()
                .map_err(|_| "selected state generation is invalid")?;
            if generation
                != applied
                    .generation
                    .checked_add(1)
                    .ok_or("state generation overflow")?
                || cycle != applied.generation
            {
                return Err("selected state generation is not the next generation".into());
            }
            let selected = match rest.get(2).copied().ok_or("state side is missing")? {
                "selected=Legacy" => "Legacy",
                "selected=Candidate" => "Candidate",
                _ => return Err("selected state side is invalid".into()),
            };
            let expected_counts = parse_state_counts(&rest, 3)?;
            pending = Some(PendingAppliedState::new(
                cycle,
                generation,
                selected,
                expected_counts,
            )?);
            continue;
        }
        if command == "STATE_ROW" {
            let rest = fields.collect::<Vec<_>>();
            if rest.len() != 3 {
                return Err("selected state row transport shape is invalid".into());
            }
            let sequence = rest
                .first()
                .copied()
                .ok_or("state sequence is missing")?
                .parse::<u64>()
                .map_err(|_| "selected state row sequence is invalid")?;
            let payload =
                decode_lower_hex_payload(rest.get(2).copied().ok_or("state payload is missing")?)?;
            pending
                .as_mut()
                .ok_or("selected state row outside a transaction")?
                .push(
                    sequence,
                    rest.get(1).copied().ok_or("state row digest is missing")?,
                    &payload,
                )?;
            continue;
        }
        if command == "STATE_COMMIT" {
            let rest = fields.collect::<Vec<_>>();
            if rest.len() != 7 {
                return Err("selected state commit shape is invalid".into());
            }
            let cycle = rest
                .first()
                .copied()
                .ok_or("state cycle is missing")?
                .parse::<u64>()
                .map_err(|_| "selected state cycle is invalid")?;
            let generation = rest
                .get(1)
                .copied()
                .ok_or("state generation is missing")?
                .parse::<u64>()
                .map_err(|_| "selected state generation is invalid")?;
            let counts = parse_state_counts(&rest, 2)?;
            let digest_index = 6;
            let declared_digest = rest
                .get(digest_index)
                .copied()
                .ok_or("selected state digest missing")?;
            let transaction = pending
                .take()
                .ok_or("selected state commit without begin")?;
            let digest =
                transaction.apply(&mut applied, cycle, generation, counts, declared_digest)?;
            let [track_table, flight_table, handoff_table, alert_table] = &applied.tables;
            let applied_counts = [
                u64::try_from(track_table.len()).map_err(|_| "applied track count overflow")?,
                u64::try_from(flight_table.len()).map_err(|_| "applied flight count overflow")?,
                u64::try_from(handoff_table.len()).map_err(|_| "applied handoff count overflow")?,
                u64::try_from(alert_table.len()).map_err(|_| "applied alert count overflow")?,
            ];
            let [tracks, flights, handoffs, alerts] = counts;
            let declared_counts = [
                u64::try_from(tracks).map_err(|_| "declared track count overflow")?,
                u64::try_from(flights).map_err(|_| "declared flight count overflow")?,
                u64::try_from(handoffs).map_err(|_| "declared handoff count overflow")?,
                u64::try_from(alerts).map_err(|_| "declared alert count overflow")?,
            ];
            if applied_counts != declared_counts {
                return Err("applied-state table counts differ from transaction".into());
            }
            if digest != applied.digest {
                return Err("applied-state digest was not retained".into());
            }
            writeln!(output, "H5CONSUMER\t1\tSTATE_APPLIED\t{cycle}\t{generation}\tselected={}\t{}\t{}\t{}\t{}\t{}",
                applied.selected, applied_counts[0], applied_counts[1], applied_counts[2], applied_counts[3], applied.digest).map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
            expected = 1;
            cycle_messages = 0;
            continue;
        }
        if command != "DATA" {
            return Err("unknown consumer command".into());
        }
        if pending.is_some() {
            return Err(
                "normalized input cannot interleave with a selected state transaction".into(),
            );
        }
        let sequence = fields
            .next()
            .ok_or("missing sequence")?
            .parse::<u64>()
            .map_err(|_| "invalid sequence")?;
        let declared_hash = fields.next().ok_or("missing payload digest")?;
        let payload_hex = fields.next().ok_or("missing payload")?;
        if fields.next().is_some() || sequence != expected {
            return Err("consumer sequence or frame shape is invalid".into());
        }
        let payload = decode_lower_hex_payload(payload_hex)?;
        let digest = oa_sha256::sha256(&payload);
        let actual_hash = oa_sha256::to_hex(&digest);
        if actual_hash != declared_hash {
            return Err("payload digest mismatch".into());
        }
        let normalized_row =
            std::str::from_utf8(&payload).map_err(|_| "normalized input payload is not UTF-8")?;
        let event = crate::input::parse_event_line(normalized_row)
            .map_err(|error| format!("normalized input row refused: {error:?}"))?;
        if !matches!(event.source, SourceKind::Surveillance | SourceKind::Flight) {
            return Err("consumer accepts only normalized surveillance/flight rows".into());
        }
        writeln!(output, "H5CONSUMER\t1\tACK\t{sequence}\t{actual_hash}")
            .map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
        expected = expected
            .checked_add(1)
            .ok_or("consumer sequence overflow")?;
        cycle_messages = cycle_messages
            .checked_add(1)
            .ok_or("consumer cycle message count overflow")?;
    }
    Ok(())
}

struct Consumer {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    replies: Receiver<Result<Vec<u8>, String>>,
    state_started: Option<Instant>,
    state_selected: Option<&'static str>,
}

impl Consumer {
    fn spawn(executable: &std::path::Path) -> Result<Self, String> {
        // seal-allow: run the pinned sibling worker with explicit stdin/stdout pipes; no file input.
        let mut child = Command::new(executable)
            .arg("--h5-consumer-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("consumer stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("consumer stdout unavailable")?;
        let (tx, replies) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut noise_bytes = 0usize;
            loop {
                match worker_read_frame(&mut reader) {
                    Ok(Some(line)) if line.starts_with(WORKER_PREFIX.as_bytes()) => {
                        if tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Ok(Some(line)) => {
                        noise_bytes = noise_bytes.saturating_add(line.len());
                        if noise_bytes > MAX_WORKER_NOISE {
                            let _ = tx.send(Err("consumer worker noise cap exceeded".into()));
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = tx.send(Err("consumer worker closed output".into()));
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(BufWriter::new(stdin)),
            replies,
            state_started: None,
            state_selected: None,
        })
    }

    fn deliver(&mut self, sequence: u64, payload: &[u8]) -> Result<ConsumerReceipt, String> {
        if payload.len() > MAX_CONSUMER_FRAME_BYTES / 2 - 128 {
            return Err("consumer payload cap exceeded".into());
        }
        let digest = oa_sha256::sha256(payload);
        let sha = oa_sha256::to_hex(&digest);
        let mut hex = String::with_capacity(payload.len().saturating_mul(2));
        for byte in payload {
            use std::fmt::Write as FmtWrite;
            write!(&mut hex, "{byte:02x}").map_err(|_| "payload hex formatting failed")?;
        }
        let line = format!("H5CONSUMER\t1\tDATA\t{sequence}\t{sha}\t{hex}\n");
        if line.len() > MAX_CONSUMER_FRAME_BYTES {
            return Err("consumer frame cap exceeded".into());
        }
        let start = Instant::now();
        let writer = self.stdin.as_mut().ok_or("consumer stdin closed")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())?;
        let response = self
            .replies
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|error| error.to_string())??;
        parse_worker_sequence(&response, sequence, &sha)?;
        Ok(ConsumerReceipt {
            sequence,
            sha256: sha,
            roundtrip_ns: start.elapsed().as_nanos(),
        })
    }

    fn end_cycle(&mut self, cycle: usize, expected_messages: u64) -> Result<(), String> {
        let line = format!("H5CONSUMER\t1\tEND_CYCLE\t{cycle}\n");
        let start = Instant::now();
        let writer = self.stdin.as_mut().ok_or("consumer stdin closed")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())?;
        let response = self
            .replies
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|error| error.to_string())??;
        let expected = format!("H5CONSUMER\t1\tCYCLE_ACK\t{cycle}\t{expected_messages}\n");
        if response != expected.as_bytes() {
            return Err("consumer cycle ACK mismatch".into());
        }
        let _ = start.elapsed();
        Ok(())
    }

    fn begin_state_cycle(
        &mut self,
        cycle: usize,
        generation: u64,
        selected: &'static str,
        counts: [u64; 4],
    ) -> Result<(), String> {
        if self.state_started.is_some() || !matches!(selected, "Legacy" | "Candidate") {
            return Err("selected state transaction is already active or side is invalid".into());
        }
        let [tracks, flights, handoffs, alerts] = counts;
        let line = format!(
            "H5CONSUMER\t1\tSTATE_BEGIN\t{cycle}\t{generation}\tselected={selected}\ttracks={}\tflights={}\thandoffs={}\talerts={}\n",
            tracks, flights, handoffs, alerts
        );
        let writer = self.stdin.as_mut().ok_or("consumer stdin closed")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())?;
        self.state_started = Some(Instant::now());
        self.state_selected = Some(selected);
        Ok(())
    }

    fn deliver_state_row(&mut self, sequence: u64, payload: &[u8]) -> Result<(), String> {
        if self.state_started.is_none() {
            return Err("selected state row outside a transaction".into());
        }
        if payload.len() > MAX_CONSUMER_FRAME_BYTES / 2 - 128 {
            return Err("selected state row cap exceeded".into());
        }
        let sha = oa_sha256::to_hex(&oa_sha256::sha256(payload));
        let hex = hex_encode(payload)?;
        let line = format!("H5CONSUMER\t1\tSTATE_ROW\t{sequence}\t{sha}\t{hex}\n");
        if line.len() > MAX_CONSUMER_FRAME_BYTES {
            return Err("selected state transport frame cap exceeded".into());
        }
        let writer = self.stdin.as_mut().ok_or("consumer stdin closed")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())
    }

    fn commit_state_cycle(
        &mut self,
        cycle: usize,
        generation: u64,
        counts: [u64; 4],
        expected_digest: &str,
    ) -> Result<ConsumerReceipt, String> {
        let start = self
            .state_started
            .take()
            .ok_or("selected state transaction is not active")?;
        let selected = self
            .state_selected
            .take()
            .ok_or("selected state side is unavailable")?;
        let [tracks, flights, handoffs, alerts] = counts;
        let line = format!(
            "H5CONSUMER\t1\tSTATE_COMMIT\t{cycle}\t{generation}\ttracks={}\tflights={}\thandoffs={}\talerts={}\t{expected_digest}\n",
            tracks, flights, handoffs, alerts
        );
        let writer = self.stdin.as_mut().ok_or("consumer stdin closed")?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| error.to_string())?;
        let response = self
            .replies
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|error| error.to_string())??;
        let expected = format!(
            "H5CONSUMER\t1\tSTATE_APPLIED\t{cycle}\t{generation}\tselected={selected}\t{}\t{}\t{}\t{}\t{expected_digest}\n",
            tracks, flights, handoffs, alerts,
        );
        if response != expected.as_bytes() {
            return Err("applied-state ACK does not match the committed selected payload".into());
        }
        Ok(ConsumerReceipt {
            sequence: generation,
            sha256: expected_digest.to_string(),
            roundtrip_ns: start.elapsed().as_nanos(),
        })
    }

    fn apply_selected_snapshot(
        &mut self,
        worker_cycle: u64,
        generation: u64,
        snapshot: SnapshotView<'_>,
        display_targets: &[oa_display::core::traffic::Target],
        output: Output,
    ) -> Result<([u64; 4], String, u64, u128), String> {
        let batch = snapshot_output_payloads(snapshot, display_targets, output)?;
        let side = match output {
            Output::Legacy => "Legacy",
            Output::Candidate => "Candidate",
        };
        let worker_cycle = usize::try_from(worker_cycle)
            .map_err(|_| "selected state cycle does not fit worker index")?;
        self.begin_state_cycle(worker_cycle, generation, side, batch.family_counts)?;
        for (row_index, payload) in batch.rows.iter().enumerate() {
            let sequence = u64::try_from(row_index)
                .map_err(|_| "selected state sequence conversion overflow")?
                .checked_add(1)
                .ok_or("selected state sequence overflow")?;
            self.deliver_state_row(sequence, payload)?;
        }
        let applied =
            self.commit_state_cycle(worker_cycle, generation, batch.family_counts, &batch.sha256)?;
        let acknowledgements = batch
            .family_counts
            .iter()
            .try_fold(0u64, |sum, count| sum.checked_add(*count))
            .ok_or("selected output ACK count overflow")?;
        Ok((
            batch.family_counts,
            applied.sha256,
            acknowledgements,
            applied.roundtrip_ns,
        ))
    }

    fn finish(&mut self) -> Result<(), String> {
        self.stdin.take();
        let deadline = Instant::now() + WORKER_CLEANUP;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("consumer exited unsuccessfully".into())
                };
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                return Err("consumer cleanup timed out".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn handoff_event(event: &crate::input::NormalizedEvent) -> bool {
    matches!(
        event.kind,
        EventKind::Flight(FlightOperation::Handoff { .. })
    )
}

fn count_event_types(events: &[crate::input::NormalizedEvent]) -> (u64, u64, u64) {
    events.iter().fold(
        (0u64, 0u64, 0u64),
        |(positions, flights, handoffs), event| match (&event.source, &event.kind) {
            (SourceKind::Surveillance, EventKind::Position { .. }) => {
                (positions.saturating_add(1), flights, handoffs)
            }
            (SourceKind::Flight, EventKind::Flight(_)) => (
                positions,
                flights.saturating_add(1),
                handoffs.saturating_add(u64::from(handoff_event(event))),
            ),
            _ => (positions, flights, handoffs),
        },
    )
}

fn policy(endpoint_class: EndpointClass) -> UnitPolicy {
    UnitPolicy {
        policy_version: 1,
        policy_digest: [3; 32],
        source_epoch_digest: [4; 32],
        update_interval_ns: match endpoint_class {
            EndpointClass::Terminal => 4_800_000_000,
            EndpointClass::EnrouteLongRange => 12_000_000_000,
        },
        max_decision_delay_ns: 1_000_000_000,
        max_snapshot_age_ns: 1_000_000_000,
        evidence_set_handle: [10; 32],
        approval_set_handle: [11; 32],
        state_schema_digest: [12; 32],
        consumer_id: 1,
        endpoint_class,
        promotion_role_mask: 1,
        controller_rollback_role_mask: 1,
        health_rollback_role_mask: 1,
        track_field_mask: 1,
        flight_field_mask: 1,
        handoff_field_mask: 1,
        alert_field_mask: 1,
    }
}

#[cfg(test)]
fn unavailable_snapshot(unit: SwapUnitKey, policy: UnitPolicy, side: Side, at_ns: i64) -> Snapshot {
    Snapshot {
        unit,
        policy,
        evidence: EvidenceKind::Recorded,
        side,
        source_epoch: match side {
            Side::Legacy => 1,
            Side::Candidate => 2,
        },
        state_version: 1,
        sequence: 1,
        source_time_ns: at_ns,
        observed_time_ns: at_ns,
        health: Health::Unknown,
        track_coverage: FamilyCoverage::Missing,
        flight_coverage: FamilyCoverage::Missing,
        handoff_coverage: FamilyCoverage::Missing,
        alert_coverage: FamilyCoverage::Missing,
        tracks: [None; MAX_TRACKS],
        flights: [None; MAX_FLIGHTS],
        handoffs: [None; MAX_HANDOFFS],
        alerts: [None; MAX_ALERTS],
    }
}

#[cfg(test)]
fn m2_refusal_for_missing_source(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    at_ns: i64,
) -> (String, u128) {
    let started = Instant::now();
    let refusal = match oa_migration_switch::new_selector(
        unit,
        policy,
        EvidenceKind::Recorded,
        unit.window_start_ns,
        &[],
    ) {
        Err(error) => format!("selector_setup:{error:?}"),
        Ok(mut selector) => match oa_migration_switch::ingest(
            &mut selector,
            unavailable_snapshot(unit, policy, Side::Legacy, at_ns),
            at_ns,
            policy.max_snapshot_age_ns,
        ) {
            Ok(()) => "unexpected_admission_of_unavailable_state".into(),
            Err(error) => format!("{error:?}"),
        },
    };
    (refusal, started.elapsed().as_nanos())
}

fn projection_key(family: EntityFamily, identity: &[u8], ordinal: u64) -> BusinessKey {
    let mut bytes = Vec::with_capacity(1 + identity.len());
    bytes.push(family as u8);
    bytes.extend_from_slice(identity);
    BusinessKey {
        entity_key: oa_sha256::sha256(&bytes),
        key_status: FieldStatus::Present,
        family,
        ordinal,
    }
}

fn sort_snapshot_rows<T: Copy>(
    rows: &mut [Option<T>],
    key: impl Fn(T) -> BusinessKey,
) -> Result<(), String> {
    rows.sort_unstable_by_key(|row| row.map(&key));
    Ok(())
}

fn projection_digest(bytes: &[u8]) -> [u8; 32] {
    oa_sha256::sha256(bytes)
}

/// Builds a bounded M2 snapshot from actual runtime output. Without the explicit local fixture,
/// external health, source identity, clock basis, full family coverage, and alert
/// acknowledgement/suppression remain unknown/partial. The opt-in fixture supplies only local
/// replay identity and an explicit ADS-B-only, empty-control-input contract; it never asserts an
/// FAA legacy/manual state or authenticated authority. Active alerts still make coverage partial.
fn snapshot_from_runtime_frame(
    frame: &RegionalFrame,
    unit: SwapUnitKey,
    policy: UnitPolicy,
    side: Side,
    source_epoch: u64,
    local_authority_fixture: bool,
) -> Result<ReplaySnapshot, String> {
    let flight_count = frame
        .flights
        .iter()
        .filter(|row| row.view.is_some())
        .count();
    let handoff_count = frame
        .flights
        .iter()
        .filter_map(|row| row.view)
        .filter(|row| row.handoff_state != oa_flight::HO_NONE)
        .count();
    let caps = [
        frame.surveillance.tracks.len(),
        flight_count,
        handoff_count,
        frame.surveillance.ca_open.len(),
    ];
    if caps.iter().any(|count| *count > REPLAY_ROWS_PER_FAMILY) {
        return Err(format!(
            "M2 replay family capacity exceeded before snapshot allocation: tracks={} flights={} handoffs={} alerts={} cap={REPLAY_ROWS_PER_FAMILY}",
            caps[0], caps[1], caps[2], caps[3]
        ));
    }
    if frame.surveillance.display_targets.len() > REPLAY_ROWS_PER_FAMILY {
        return Err("regional display block count exceeds the explicit H5 consumer cap".into());
    }
    let mut display_targets = frame.surveillance.display_targets.clone();
    display_targets.sort_unstable_by_key(|target| target.addr);
    if display_targets
        .windows(2)
        .any(|pair| pair.first().map(|row| row.addr) == pair.get(1).map(|row| row.addr))
    {
        return Err("regional display builder produced duplicate address blocks".into());
    }
    let local_stream = if local_authority_fixture {
        u32::try_from(source_epoch).map_err(|_| "local replay stream identity overflow")?
    } else {
        0
    };
    let mut tracks = Vec::new();
    tracks
        .try_reserve_exact(caps[0])
        .map_err(|_| "M2 track snapshot allocation refused")?;
    let mut flights = Vec::new();
    flights
        .try_reserve_exact(caps[1])
        .map_err(|_| "M2 flight snapshot allocation refused")?;
    let mut handoffs = Vec::new();
    handoffs
        .try_reserve_exact(caps[2])
        .map_err(|_| "M2 handoff snapshot allocation refused")?;
    let mut alerts = Vec::new();
    alerts
        .try_reserve_exact(caps[3])
        .map_err(|_| "M2 alert snapshot allocation refused")?;
    let mut track_count = 0usize;
    let mut flight_count = 0usize;
    let mut handoff_count = 0usize;
    let mut alert_count = 0usize;
    for row in &frame.surveillance.tracks {
        let mut identity = [0u8; 4];
        identity.copy_from_slice(&row.address.to_be_bytes());
        let key = projection_key(EntityFamily::Track, &identity, u64::from(row.address));
        let mut bytes = Vec::with_capacity(48);
        bytes.extend_from_slice(&row.state_ns.to_le_bytes());
        bytes.extend_from_slice(&row.newest_source_ns.to_le_bytes());
        bytes.extend_from_slice(&row.address.to_le_bytes());
        bytes.extend_from_slice(&row.lat_e7.to_le_bytes());
        bytes.extend_from_slice(&row.lon_e7.to_le_bytes());
        bytes.extend_from_slice(&row.altitude_ft.to_le_bytes());
        bytes.extend_from_slice(&row.vertical_rate_fpm.to_le_bytes());
        bytes.extend_from_slice(&row.east_mmps.to_le_bytes());
        bytes.extend_from_slice(&row.north_mmps.to_le_bytes());
        bytes.extend_from_slice(&row.r95_mm.to_le_bytes());
        tracks.push(Some(TrackState {
            key,
            source_identity: SourceIdentity {
                stream: local_stream,
                decoded_offset: 0,
                row: u32::try_from(track_count).map_err(|_| "track row identity overflow")?,
                operation_index: 0,
            },
            time_basis_id: u32::from(local_authority_fixture),
            source_time_ns: if local_authority_fixture {
                row.newest_source_ns
            } else {
                0
            },
            observed_time_ns: frame.surveillance.time_ns,
            status: if local_authority_fixture {
                FieldStatus::Present
            } else {
                FieldStatus::Unknown
            },
            lat_e7: row.lat_e7,
            lon_e7: row.lon_e7,
            altitude_ft: row.altitude_ft,
            speed_dkt: 0,
            track_ddeg: 0,
            flags: 0,
            field_mask: if local_authority_fixture {
                policy.track_field_mask
            } else {
                0
            },
            projection_digest: projection_digest(&bytes),
        }));
        track_count += 1;
    }
    for row in frame.flights.iter().filter_map(|row| row.view) {
        let identity = row.gufi.to_be_bytes();
        let key = projection_key(EntityFamily::Flight, &identity, row.gufi as u64);
        let mut digest_bytes = Vec::with_capacity(64);
        digest_bytes.extend_from_slice(&row.plan_id);
        digest_bytes.push(row.plan_state);
        digest_bytes.extend_from_slice(&row.callsign);
        digest_bytes.extend_from_slice(&row.departure);
        digest_bytes.extend_from_slice(&row.arrival);
        digest_bytes.extend_from_slice(&row.address.to_le_bytes());
        digest_bytes.extend_from_slice(&row.owner_unit);
        digest_bytes.push(row.handoff_state);
        digest_bytes.extend_from_slice(&row.handoff_from);
        digest_bytes.extend_from_slice(&row.handoff_to);
        digest_bytes.extend_from_slice(&row.handoff_accepted_by);
        flights.push(Some(FlightState {
            key,
            source_identity: SourceIdentity {
                stream: local_stream,
                decoded_offset: 0,
                row: u32::try_from(flight_count).map_err(|_| "flight row identity overflow")?,
                operation_index: 0,
            },
            time_basis_id: u32::from(local_authority_fixture),
            source_time_ns: if local_authority_fixture {
                frame.surveillance.time_ns
            } else {
                0
            },
            observed_time_ns: frame.surveillance.time_ns,
            status: if local_authority_fixture {
                FieldStatus::Present
            } else {
                FieldStatus::Unknown
            },
            flight_plan_digest: projection_digest(&digest_bytes),
            flags: u32::from(row.plan_state),
            field_mask: if local_authority_fixture {
                policy.flight_field_mask
            } else {
                0
            },
            projection_digest: projection_digest(&digest_bytes),
        }));
        flight_count += 1;
        if row.handoff_state != oa_flight::HO_NONE {
            let handoff_key = projection_key(EntityFamily::Handoff, &identity, row.gufi as u64);
            let mut handoff_digest = Vec::with_capacity(40);
            handoff_digest.extend_from_slice(&identity);
            handoff_digest.push(row.handoff_state);
            handoff_digest.extend_from_slice(&row.handoff_from);
            handoff_digest.extend_from_slice(&row.handoff_to);
            handoff_digest.extend_from_slice(&row.handoff_accepted_by);
            handoffs.push(Some(HandoffState {
                key: handoff_key,
                source_identity: SourceIdentity {
                    stream: local_stream,
                    decoded_offset: 0,
                    row: u32::try_from(handoff_count)
                        .map_err(|_| "handoff row identity overflow")?,
                    operation_index: 0,
                },
                time_basis_id: u32::from(local_authority_fixture),
                source_time_ns: if local_authority_fixture {
                    frame.surveillance.time_ns
                } else {
                    0
                },
                observed_time_ns: frame.surveillance.time_ns,
                status: if local_authority_fixture {
                    FieldStatus::Present
                } else {
                    FieldStatus::Unknown
                },
                lifecycle: row.handoff_state,
                owner: u32::from_le_bytes(
                    row.handoff_from[..4]
                        .try_into()
                        .map_err(|_| "handoff owner projection width")?,
                ),
                recipient: u32::from_le_bytes(
                    row.handoff_to[..4]
                        .try_into()
                        .map_err(|_| "handoff recipient projection width")?,
                ),
                flags: 0,
                field_mask: if local_authority_fixture {
                    policy.handoff_field_mask
                } else {
                    0
                },
                projection_digest: projection_digest(&handoff_digest),
            }));
            handoff_count += 1;
        }
    }
    for row in &frame.surveillance.ca_open {
        let mut identity = [0u8; 8];
        identity[..4].copy_from_slice(&row.addr_a.to_be_bytes());
        identity[4..].copy_from_slice(&row.addr_b.to_be_bytes());
        let ordinal = (u64::from(row.addr_a) << 32) | u64::from(row.addr_b);
        let key = projection_key(EntityFamily::Alert, &identity, ordinal);
        let mut digest_bytes = Vec::with_capacity(40);
        digest_bytes.extend_from_slice(&identity);
        digest_bytes.extend_from_slice(&row.start_ns.to_le_bytes());
        digest_bytes.extend_from_slice(&row.end_ns.to_le_bytes());
        digest_bytes.extend_from_slice(&row.peak_ns.to_le_bytes());
        digest_bytes.extend_from_slice(&row.flags.to_le_bytes());
        alerts.push(Some(AlertState {
            key,
            source_identity: SourceIdentity {
                stream: local_stream,
                decoded_offset: 0,
                row: u32::try_from(alert_count).map_err(|_| "alert row identity overflow")?,
                operation_index: 0,
            },
            time_basis_id: u32::from(local_authority_fixture),
            source_time_ns: if local_authority_fixture {
                frame.surveillance.time_ns
            } else {
                0
            },
            observed_time_ns: frame.surveillance.time_ns,
            status: FieldStatus::Unknown,
            active: 1,
            // Zero-initialized fields are explicitly not known false; `status=Unknown` and a
            // zero field mask keep acknowledgement/suppression unavailable to M2.
            acknowledged: 0,
            suppressed: 0,
            kind: u32::from(row.flags),
            field_mask: 0,
            projection_digest: projection_digest(&digest_bytes),
        }));
        alert_count += 1;
    }
    let mut snapshot = ReplaySnapshot {
        unit,
        policy,
        evidence: EvidenceKind::Recorded,
        side,
        source_epoch,
        state_version: frame.sequence,
        sequence: frame.sequence,
        source_time_ns: frame.surveillance.time_ns,
        observed_time_ns: frame.surveillance.time_ns,
        health: Health::Unknown,
        track_coverage: FamilyCoverage::Missing,
        flight_coverage: FamilyCoverage::Missing,
        handoff_coverage: FamilyCoverage::Missing,
        alert_coverage: FamilyCoverage::Missing,
        tracks,
        flights,
        handoffs,
        alerts,
        display_targets,
    };
    let local_replay_healthy = local_authority_fixture
        && caps.iter().all(|count| *count <= REPLAY_ROWS_PER_FAMILY)
        && frame.surveillance.ca_work_short == 0
        && frame.surveillance.ca_input_short == 0
        && frame.surveillance.ca_stats.tracks_unscanned == 0
        && frame.surveillance.ca_stats.rows_bad == 0
        && frame.surveillance.ca_stats.rows_duplicate == 0
        && frame.surveillance.ca_stats.rows_over_capacity == 0
        && frame.surveillance.ca_stats.candidates_dropped == 0
        && frame.surveillance.measurement_rejections == 0
        && frame
            .surveillance
            .tracker_stats
            .refused
            .iter()
            .all(|count| *count == 0)
        && frame.surveillance.tracker_stats.capacity_refused == 0
        && frame.surveillance.tracker_stats.numerical_refused == 0
        && frame.flight_report.refusals_unlisted == 0
        && frame.flight_report.conflicts_unlisted == 0
        && frame.quarantine.records.is_empty()
        && frame.quarantine.source_stats.iter().all(|stats| {
            stats.queued == 0
                && stats.queue_full_manual_events == 0
                && stats.reason_counts.iter().all(|count| *count == 0)
        });
    sort_snapshot_rows(&mut snapshot.tracks, |row| row.key)?;
    sort_snapshot_rows(&mut snapshot.flights, |row| row.key)?;
    sort_snapshot_rows(&mut snapshot.handoffs, |row| row.key)?;
    sort_snapshot_rows(&mut snapshot.alerts, |row| row.key)?;
    snapshot.health = if local_replay_healthy {
        Health::Healthy
    } else {
        Health::Unknown
    };
    let complete = |count: usize| {
        if local_replay_healthy {
            if count == 0 {
                FamilyCoverage::CompleteEmpty
            } else {
                FamilyCoverage::CompleteWithEvents
            }
        } else {
            FamilyCoverage::Partial
        }
    };
    snapshot.track_coverage = complete(track_count);
    snapshot.flight_coverage = complete(flight_count);
    snapshot.handoff_coverage = complete(handoff_count);
    snapshot.alert_coverage = if alert_count == 0 {
        complete(alert_count)
    } else {
        // This local replay has no acknowledgement/suppression input family. Active alerts
        // therefore remain unknown and make M2 refuse the snapshot.
        FamilyCoverage::Partial
    };
    let _represented = (track_count, flight_count, handoff_count, alert_count);
    Ok(snapshot)
}

fn m2_attempt_for_runtime_snapshots(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    at_ns: i64,
    legacy: SnapshotView<'_>,
    candidate: SnapshotView<'_>,
) -> (String, u128, Output) {
    let started = Instant::now();
    let (refusal, selected_output) = match new_selector_with_capacity(
        unit,
        policy,
        EvidenceKind::Recorded,
        unit.window_start_ns,
        &[],
        CapacityProfile::REPLAY_4096,
    ) {
        Err(error) => (format!("selector_setup:{error:?}"), Output::Legacy),
        Ok(mut selector) => {
            let legacy_result =
                ingest_view(&mut selector, legacy, at_ns, policy.max_snapshot_age_ns);
            let candidate_result =
                ingest_view(&mut selector, candidate, at_ns, policy.max_snapshot_age_ns);
            let refusal = match (legacy_result, candidate_result) {
                (Err(legacy), Err(candidate)) => {
                    format!("legacy={legacy:?};candidate={candidate:?}")
                }
                (Err(legacy), Ok(())) => format!("legacy={legacy:?};candidate=admitted"),
                (Ok(()), Err(candidate)) => format!("legacy=admitted;candidate={candidate:?}"),
                (Ok(()), Ok(())) => {
                    let mut diffs = Vec::new();
                    let diff_capacity = CapacityProfile::REPLAY_4096.tracks * 8;
                    match diffs.try_reserve_exact(diff_capacity) {
                        Err(_) => "profile_diff_allocation_refused".into(),
                        Ok(()) => {
                            diffs.resize(diff_capacity, None::<Diff>);
                            match prepare_candidate_into(
                                &selector,
                                at_ns,
                                policy.max_snapshot_age_ns,
                                &mut diffs,
                            ) {
                                Ok(_) => {
                                    "candidate_ready_but_m1_window_authorization_unavailable".into()
                                }
                                Err(error) => format!("prepare_candidate:{error:?}"),
                            }
                        }
                    }
                }
            };
            (refusal, selected(&selector))
        }
    };
    (refusal, started.elapsed().as_nanos(), selected_output)
}

/// Issues the existing M1 window receipt from a fixed local mechanism-test fixture. Principal
/// identifiers, approvals, and metric evidence here are not authenticated FAA authority or
/// operational evidence; callers must label the run as a same-implementation mechanism exercise.
fn local_fixture_window_authorization(
    selector: &oa_migration_switch::Selector,
    unit: SwapUnitKey,
    policy: UnitPolicy,
    at_ns: i64,
) -> Result<oa_migration_switch::WindowAuthorization, String> {
    let key = M1SwapUnitKey {
        function_id: unit.function_id,
        scope_id: unit.scope_id,
        window_start_ns: unit.window_start_ns,
        window_end_ns: unit.window_end_ns,
    };
    let unit_m1 = new_unit(
        key,
        policy.policy_version,
        policy.policy_digest,
        policy.source_epoch_digest,
        unit.window_start_ns,
    )
    .map_err(|error| format!("local fixture M1 unit: {error:?}"))?;
    let first_at = unit
        .window_start_ns
        .checked_add(2)
        .ok_or("local fixture M1 timestamp overflow")?;
    if at_ns <= first_at {
        return Err("local fixture M1 evidence window is empty".into());
    }
    let criteria = [MetricCriterion {
        metric_id: 4,
        comparison: Comparison::AtMost,
        threshold: 3,
        minimum_samples: 1,
        minimum_denominator: 1,
        minimum_exposure_ns: 1,
    }; oa_migration::MAX_CRITERIA];
    let rollback_policy = RollbackTriggerPolicy {
        blocking_difference: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: ROLE_SUPERVISOR,
        },
        controller_request: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: ROLE_CONTROLLER_ON_POSITION,
        },
        health_fault: RollbackRule {
            enabled: true,
            maximum_response_ns: 1_000_000_000,
            required_role_mask: ROLE_TECH_OPS,
        },
    };
    let build_policy = |from, to| TransitionPolicy {
        key,
        from,
        to,
        policy_version: policy.policy_version,
        policy_digest: policy.policy_digest,
        source_epoch_digest: policy.source_epoch_digest,
        maximum_evidence_age_ns: 10_000_000_000,
        required_role_mask: ROLE_SUPERVISOR | ROLE_ASSESSOR,
        criterion_count: 1,
        criteria,
        dependency_count: 0,
        dependencies: [DependencyRequirement {
            key,
            minimum_stage: Stage::S0Shadow,
            policy_version: policy.policy_version,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
        }; oa_migration::MAX_DEPENDENCIES],
        rollback_policy,
    };
    let build_evidence =
        |stage: Stage, start_ns: i64, end_ns: i64| -> Result<[MetricEvidence; 1], String> {
            let exposure = end_ns
                .checked_sub(start_ns)
                .and_then(|span| u64::try_from(span).ok())
                .ok_or("local fixture M1 evidence exposure invalid")?;
            Ok([MetricEvidence {
                key,
                measured_stage: stage,
                metric_id: 4,
                quality: EvidenceQuality::Complete,
                policy_version: policy.policy_version,
                samples: 5,
                denominator: 8,
                observed: 2,
                exposure_ns: exposure,
                observation_start_ns: start_ns,
                observation_end_ns: end_ns,
                observed_at_ns: end_ns,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                artifact_handle: [20; 32],
                evidence_set_handle: policy.evidence_set_handle,
                observation_origin_key: key,
                origin_stage_activation_ns: 0,
                origin_receipt_handle: [0; 32],
            }])
        };
    let approvals = |from, to, at| {
        [
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::FacilitySupervisor,
                approved: true,
                principal_id: 8,
                approved_at_ns: at,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [21; 32],
            },
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::IndependentAssessor,
                approved: true,
                principal_id: 9,
                approved_at_ns: at,
                policy_version: policy.policy_version,
                policy_digest: policy.policy_digest,
                source_epoch_digest: policy.source_epoch_digest,
                evidence_set_handle: policy.evidence_set_handle,
                approval_set_handle: policy.approval_set_handle,
                authority_record_handle: [22; 32],
            },
        ]
    };
    let mut audit = empty_audit_log();
    let first_policy = build_policy(Stage::S0Shadow, Stage::S1Advisory);
    let first_evidence = build_evidence(Stage::S0Shadow, unit.window_start_ns, first_at - 1)?;
    let first_approvals = approvals(Stage::S0Shadow, Stage::S1Advisory, first_at);
    let first = evaluate_transition(
        &unit_m1,
        TransitionInput {
            policy: &first_policy,
            metrics: &first_evidence,
            dependencies: &[],
            approvals: &first_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: first_at,
        },
        &mut audit,
    );
    if oa_migration::transition_event(&first).refusal != oa_migration::Refusal::None {
        return Err("local fixture M1 S0-to-S1 transition refused".into());
    }
    let second_policy = build_policy(Stage::S1Advisory, Stage::S2Window);
    let second_evidence = build_evidence(Stage::S1Advisory, first_at, at_ns - 1)?;
    let second_approvals = approvals(Stage::S1Advisory, Stage::S2Window, at_ns);
    let second = evaluate_transition(
        &oa_migration::transition_unit(&first),
        TransitionInput {
            policy: &second_policy,
            metrics: &second_evidence,
            dependencies: &[],
            approvals: &second_approvals,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            now_ns: at_ns,
        },
        &mut audit,
    );
    if oa_migration::transition_event(&second).refusal != oa_migration::Refusal::None {
        return Err("local fixture M1 S1-to-S2 transition refused".into());
    }
    authorize_window(selector, &second)
        .map_err(|error| format!("local fixture M1 window authorization: {error:?}"))
}

struct LocalFixtureCandidate {
    selector: oa_migration_switch::Selector,
    m2_validation_ns: u128,
    m1_fixture_ns: u128,
}

fn prepare_local_fixture_candidate(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    at_ns: i64,
    legacy: SnapshotView<'_>,
    candidate: SnapshotView<'_>,
    cycle: usize,
) -> Result<LocalFixtureCandidate, String> {
    let m2_started = Instant::now();
    let mut selector = new_selector_with_capacity(
        unit,
        policy,
        EvidenceKind::Recorded,
        unit.window_start_ns,
        &[],
        CapacityProfile::REPLAY_4096,
    )
    .map_err(|error| format!("local fixture selector: {error:?}"))?;
    ingest_view(&mut selector, legacy, at_ns, policy.max_snapshot_age_ns)
        .map_err(|error| format!("local fixture legacy snapshot: {error:?}"))?;
    ingest_view(&mut selector, candidate, at_ns, policy.max_snapshot_age_ns)
        .map_err(|error| format!("local fixture candidate snapshot: {error:?}"))?;
    let mut diffs = Vec::new();
    let diff_capacity = CapacityProfile::REPLAY_4096.tracks * 8;
    diffs
        .try_reserve_exact(diff_capacity)
        .map_err(|_| "local fixture diff allocation refused")?;
    diffs.resize(diff_capacity, None::<Diff>);
    let (reconciliation, readiness) =
        prepare_candidate_into(&selector, at_ns, policy.max_snapshot_age_ns, &mut diffs)
            .map_err(|error| format!("local fixture candidate readiness: {error:?}"))?;
    if !reconciliation.ready || reconciliation.differences != 0 {
        return Err("same-implementation replay projections do not reconcile".into());
    }
    let m2_before_authority_ns = m2_started.elapsed().as_nanos();
    let m1_started = Instant::now();
    let authorization = local_fixture_window_authorization(&selector, unit, policy, at_ns)?;
    let m1_fixture_ns = m1_started.elapsed().as_nanos();
    let mut event_material = Vec::with_capacity(32);
    event_material.extend_from_slice(b"H5 local fixture promotion event v1");
    event_material.extend_from_slice(
        &u64::try_from(cycle)
            .map_err(|_| "cycle overflow")?
            .to_le_bytes(),
    );
    let event_key = oa_sha256::sha256(&event_material);
    let intent = new_promotion_intent(
        &selector,
        &readiness,
        &authorization,
        PromotionIntentRequest {
            sequence: 1,
            evaluated_at_ns: at_ns,
            principal_role_id: 1,
            principal_id: 8,
            event_key,
        },
    )
    .map_err(|error| format!("local fixture candidate intent: {error:?}"))?;
    let select_started = Instant::now();
    select_candidate(
        &mut selector,
        &readiness,
        &authorization,
        &intent,
        at_ns,
        policy.max_snapshot_age_ns,
    )
    .map_err(|error| format!("local fixture candidate selection: {error:?}"))?;
    Ok(LocalFixtureCandidate {
        selector,
        m2_validation_ns: m2_before_authority_ns
            .saturating_add(select_started.elapsed().as_nanos()),
        m1_fixture_ns,
    })
}

fn selected_output_name(output: Output) -> &'static str {
    match output {
        Output::Legacy => "legacy_default_or_retained",
        Output::Candidate => "candidate",
    }
}

fn m1_refusal_without_authority(key: oa_migration::SwapUnitKey, at_ns: i64) -> (String, u128) {
    let started = Instant::now();
    let digest = [3; 32];
    let epoch = [4; 32];
    let refusal = match new_unit(key, 1, digest, epoch, at_ns) {
        Err(error) => format!("unit_setup:{error:?}"),
        Ok(unit) => {
            let policy = TransitionPolicy {
                key,
                from: Stage::S0Shadow,
                to: Stage::S1Advisory,
                policy_version: 1,
                policy_digest: digest,
                source_epoch_digest: epoch,
                maximum_evidence_age_ns: 1_000_000_000,
                required_role_mask: ROLE_SUPERVISOR | ROLE_ASSESSOR,
                criterion_count: 1,
                criteria: [MetricCriterion {
                    metric_id: 4,
                    comparison: Comparison::AtMost,
                    threshold: 1,
                    minimum_samples: 1,
                    minimum_denominator: 1,
                    minimum_exposure_ns: 1,
                }; oa_migration::MAX_CRITERIA],
                dependency_count: 0,
                dependencies: [DependencyRequirement {
                    key,
                    minimum_stage: Stage::S0Shadow,
                    policy_version: 1,
                    policy_digest: digest,
                    source_epoch_digest: epoch,
                }; oa_migration::MAX_DEPENDENCIES],
                rollback_policy: RollbackTriggerPolicy {
                    blocking_difference: RollbackRule {
                        enabled: true,
                        maximum_response_ns: 1_000_000_000,
                        required_role_mask: ROLE_SUPERVISOR,
                    },
                    controller_request: RollbackRule {
                        enabled: true,
                        maximum_response_ns: 1_000_000_000,
                        required_role_mask: ROLE_CONTROLLER_ON_POSITION,
                    },
                    health_fault: RollbackRule {
                        enabled: true,
                        maximum_response_ns: 1_000_000_000,
                        required_role_mask: ROLE_TECH_OPS,
                    },
                },
            };
            let no_metrics = [];
            let no_dependencies: [oa_migration::DependencyEvidence; 0] = [];
            let no_approvals: [AuthorityApproval; 0] = [];
            let mut audit = empty_audit_log();
            let result = evaluate_transition(
                &unit,
                TransitionInput {
                    policy: &policy,
                    metrics: &no_metrics,
                    dependencies: &no_dependencies,
                    approvals: &no_approvals,
                    evidence_set_handle: [10; 32],
                    approval_set_handle: [11; 32],
                    now_ns: at_ns,
                },
                &mut audit,
            );
            format!("{:?}", oa_migration::transition_event(&result).refusal)
        }
    };
    (refusal, started.elapsed().as_nanos())
}

/// Read/normalize a bounded recording, ACK its actual normalized rows in a separate process, and
/// attempt (then truthfully refuse) each of 100 time bins unless complete system snapshots exist.
pub fn run_recorded(
    inputs: &[RecordingInput],
    centre: [u8; 4],
    endpoint_class: EndpointClass,
    model_bytes: &[u8],
    map_range_nm: f64,
    worker_executable: &std::path::Path,
) -> Result<RecordedDrillReport, String> {
    let run_started = Instant::now();
    let converted = convert_recordings(inputs, centre)
        .map_err(|error| format!("recording read/normalize: {error:?}"))?;
    let events = &converted.adaptation.events;
    if events.len() > MAX_NORMALIZED_EVENTS {
        return Err("normalized event cap exceeded".into());
    }
    let (positions, flights, handoffs) = count_event_types(events);
    let rows = converted.normalized_tsv.lines().skip(1).collect::<Vec<_>>();
    if rows.len() != events.len() {
        return Err("normalized line/event count mismatch".into());
    }
    let mut consumer = Consumer::spawn(worker_executable)?;
    let mut receipts = Vec::with_capacity(rows.len());
    let mut per_cycle_ack = [0u64; CYCLES];
    let mut per_cycle_max = [0u128; CYCLES];
    let (start, end) =
        event_span(events).ok_or("recording contains no normalized source events")?;
    for (row_index, row) in rows.iter().enumerate() {
        if run_started.elapsed() > MAX_RUNTIME {
            return Err("recorded drill runtime cap exceeded".into());
        }
        let event = events
            .get(row_index)
            .ok_or("normalized event index mismatch")?;
        let payload = row.as_bytes();
        let sequence = u64::try_from(row_index)
            .map_err(|_| "consumer sequence overflow")?
            .checked_add(1)
            .ok_or("consumer sequence overflow")?;
        let receipt = consumer.deliver(sequence, payload)?;
        let cycle = row_cycle_in_span(event.receipt_ns, start, end)?;
        let acks = per_cycle_ack
            .get_mut(cycle)
            .ok_or("cycle index out of range")?;
        *acks = acks.checked_add(1).ok_or("consumer ACK count overflow")?;
        let max = per_cycle_max
            .get_mut(cycle)
            .ok_or("cycle index out of range")?;
        *max = (*max).max(receipt.roundtrip_ns);
        receipts.push(receipt);
    }
    consumer.finish()?;
    let model_sha = oa_sha256::sha256(model_bytes);
    let model = oa_airspace::core::snapshot::decode(model_bytes)
        .map_err(|error| format!("model snapshot decode: {error:?}"))?;
    let (map_centre_lat_e7, map_centre_lon_e7) = model_centre(&model)?;
    drop(model);
    // H5 records the exact per-frame targets produced by the existing regional display builder.
    // Its explicit 8,192-presentation cap remains a hard refusal, not a sampling limit.
    let run_profile = RegionalRunProfile::recorded_peak_rendered();
    let snapshot_policy = policy(endpoint_class);
    let baseline = replay_replica(
        events,
        ReplaySpec {
            model_bytes,
            model_sha,
            range_nm: map_range_nm,
            capture_start: start,
            start,
            end,
            profile: run_profile,
            snapshot_policy,
            side: Side::Legacy,
            source_epoch: 1,
            local_authority_fixture: false,
        },
    )?;
    if run_started.elapsed() > MAX_RUNTIME {
        return Err("recorded drill runtime cap exceeded".into());
    }
    let candidate = replay_replica(
        events,
        ReplaySpec {
            model_bytes,
            model_sha,
            range_nm: map_range_nm,
            capture_start: start,
            start,
            end,
            profile: run_profile,
            snapshot_policy,
            side: Side::Candidate,
            source_epoch: 2,
            local_authority_fixture: false,
        },
    )?;
    let max_retained_snapshot_bytes_per_replica = baseline
        .retained_snapshot_bytes
        .max(candidate.retained_snapshot_bytes);
    let snapshot_sets = [baseline.snapshots, candidate.snapshots];
    let replica_cycles = [baseline.cycles, candidate.cycles];
    let mut replay_consumer = Consumer::spawn(worker_executable)?;
    let mut replay_consumer_receipts = Vec::with_capacity(CYCLES * 2);
    let mut replay_cycle_max = [0u128; CYCLES];
    for index in 0..CYCLES {
        for pass in 0..2 {
            let row = replica_cycles
                .get(pass)
                .and_then(|cycles| cycles.get(index))
                .ok_or("replay projection cycle missing")?;
            let payload = format!(
                "OA-REGIONAL-REPLAY\t1\tpass={pass}\tcycle={index}\tframes={}\ttracks={}\tflights={}\thandoffs={}\tactive_ca_alerts={}\tca_work_short={}\tca_input_short={}\tca_tracks_unscanned={}\tquarantined={}\tmanual_handling={}\tlast_frame_ns={}\tprojection_sha256={}",
                row.frames,
                row.track_rows,
                row.flight_views,
                row.handoff_views,
                row.active_ca_alerts,
                row.ca_work_short,
                row.ca_input_short,
                row.ca_tracks_unscanned,
                row.quarantined_items,
                row.manual_handling_items,
                row.last_frame_ns
                    .map_or_else(|| "UNAVAILABLE".into(), |value| value.to_string()),
                row.projection_sha256,
            );
            let sequence = u64::try_from(pass + 1).map_err(|_| "replay sequence overflow")?;
            let receipt = replay_consumer.deliver(sequence, payload.as_bytes())?;
            let max = replay_cycle_max
                .get_mut(index)
                .ok_or("replay cycle max index")?;
            *max = (*max).max(receipt.roundtrip_ns);
            replay_consumer_receipts.push(receipt);
        }
        replay_consumer.end_cycle(index, 2)?;
    }
    replay_consumer.finish()?;
    let span = i128::from(end)
        .checked_sub(i128::from(start))
        .ok_or("recorded time span overflow")?;
    let mut cycles = Vec::with_capacity(CYCLES);
    for index in 0..CYCLES {
        let lo = i128::from(start) + span * index as i128 / CYCLES as i128;
        let hi = i128::from(start) + span * (index + 1) as i128 / CYCLES as i128;
        let start_ns = i64::try_from(lo).map_err(|_| "cycle start overflow")?;
        let end_ns = i64::try_from(hi).map_err(|_| "cycle end overflow")?;
        let mut pos = 0u64;
        let mut flight = 0u64;
        let mut handoff = 0u64;
        for event in events
            .iter()
            .filter(|event| event.receipt_ns >= start_ns && event.receipt_ns < end_ns)
        {
            match (&event.source, &event.kind) {
                (SourceKind::Surveillance, EventKind::Position { .. }) => {
                    pos = pos.saturating_add(1)
                }
                (SourceKind::Flight, EventKind::Flight(_)) => {
                    flight = flight.saturating_add(1);
                    handoff = handoff.saturating_add(u64::from(handoff_event(event)));
                }
                _ => (),
            }
        }
        let unit = SwapUnitKey {
            function_id: 1,
            scope_id: 1,
            window_start_ns: start_ns,
            window_end_ns: end_ns.max(start_ns.saturating_add(1)),
        };
        let legacy_snapshot = snapshot_sets[0]
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("baseline runtime snapshot missing")?
            .clone();
        let candidate_snapshot = snapshot_sets[1]
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("candidate runtime snapshot missing")?
            .clone();
        let snapshot_now_ns = legacy_snapshot
            .observed_time_ns
            .max(candidate_snapshot.observed_time_ns);
        let (refusal, m2_validation_ns, selected_output) = m2_attempt_for_runtime_snapshots(
            unit,
            snapshot_policy,
            snapshot_now_ns,
            legacy_snapshot.view(),
            candidate_snapshot.view(),
        );
        let m1_key = oa_migration::SwapUnitKey {
            function_id: unit.function_id,
            scope_id: unit.scope_id,
            window_start_ns: unit.window_start_ns,
            window_end_ns: unit.window_end_ns,
        };
        let (m1_refusal, m1_validation_ns) = m1_refusal_without_authority(m1_key, start_ns);
        let left = replica_cycles[0]
            .get(index)
            .ok_or("baseline projection missing")?;
        let right = replica_cycles[1]
            .get(index)
            .ok_or("second projection missing")?;
        let input_acks = *per_cycle_ack.get(index).ok_or("cycle ACK index")?;
        let input_max_ack = *per_cycle_max.get(index).ok_or("cycle ACK max index")?;
        cycles.push(CycleAttempt {
            index, start_ns, end_ns, position_inputs: pos, flight_inputs: flight,
            handoff_operations: handoff,
            consumer_acks: input_acks,
            consumer_max_ack_ns: if input_acks == 0 { None } else { Some(input_max_ack) },
            replay_output_max_ack_ns: *replay_cycle_max.get(index).ok_or("replay cycle ACK index")?,
            replay_output_acks: 2,
            selected_state_applied_generation: None,
            selected_state_applied_family_counts: None,
            selected_display_block_rows: None,
            display_targets_equal: display_targets_equal(
                &legacy_snapshot.display_targets,
                &candidate_snapshot.display_targets,
            ),
            selected_state_applied_sha256: None,
            decision_to_selected_output_ack_ns: None,
            rollback_output_apply_ack_ns: None,
            flip_rollback_attempted: false,
            selected_output: selected_output_name(selected_output),
            selector_export_dropped_baseline: left.selector_export_dropped,
            selector_export_dropped_candidate: right.selector_export_dropped,
            replay_projection_equal: left.frames > 0 && right.frames > 0
                && left.projection_sha256 == right.projection_sha256,
            baseline_projection_sha256: left.projection_sha256.clone(),
            second_projection_sha256: right.projection_sha256.clone(),
            baseline_track_rows: left.track_rows, second_track_rows: right.track_rows,
            baseline_flight_views: left.flight_views, second_flight_views: right.flight_views,
            baseline_handoff_views: left.handoff_views, second_handoff_views: right.handoff_views,
            baseline_active_ca_alerts: left.active_ca_alerts,
            second_active_ca_alerts: right.active_ca_alerts,
            m2_ingest_refusal: Some(refusal), m2_validation_ns: Some(m2_validation_ns),
            m1_transition_refusal: Some(m1_refusal), m1_validation_ns: Some(m1_validation_ns),
            flip_rollback_status: "NOT_PERFORMED_INCOMPLETE_STATE",
            unavailable: "same-implementation replay sides; selector health, source locator/time basis, complete family coverage, alert acknowledgement/suppression and M1 window authority are unavailable",
            consumer_frame_updates: 0,
            consumer_max_virtual_gap_ns: None,
            consumer_max_apply_ack_ns: None,
        });
    }
    let input_streams = converted
        .streams
        .iter()
        .map(|stream| StreamReport {
            source: match stream.source {
                RecordingSource::Adsb => "ADSB",
                RecordingSource::Sfdps => "SFDPS",
            },
            sha256: oa_sha256::to_hex(&stream.sha256),
            bytes: stream.compressed_bytes,
            messages: 0,
            first_frame_receive_ns: None,
            last_frame_receive_ns: None,
            max_frame_gap_ns: 0,
            source_full_window_covered: false,
            source_largest_gap_ns: 0,
        })
        .collect();
    let record_counts = converted.adaptation.recording;
    let invalid_source_frame_count = record_counts
        .adsb
        .invalid_frames
        .checked_add(record_counts.sfdps.invalid_frames)
        .ok_or("invalid frame count overflow")?;
    let truncated_source_frame_count = record_counts
        .adsb
        .truncated_frames
        .checked_add(record_counts.sfdps.truncated_frames)
        .ok_or("truncated frame count overflow")?;
    let skipped_source_bytes = record_counts
        .adsb
        .skipped_bytes
        .checked_add(record_counts.sfdps.skipped_bytes)
        .ok_or("skipped byte count overflow")?;
    let source_files_refused = record_counts
        .adsb
        .files_refused
        .checked_add(record_counts.sfdps.files_refused)
        .ok_or("refused file count overflow")?;
    let quarantine_record_count = u64::try_from(converted.adaptation.quarantine.len())
        .map_err(|_| "quarantine count overflow")?;
    let manual_handling_required_count = u64::try_from(
        converted
            .adaptation
            .quarantine
            .iter()
            .filter(|row| row.manual_handling_required)
            .count(),
    )
    .map_err(|_| "manual handling count overflow")?;
    let m2_cycle_count = u64::try_from(cycles.len()).map_err(|_| "M2 cycle count overflow")?;
    let m2_refusal_count = u64::try_from(
        cycles
            .iter()
            .filter(|cycle| cycle.m2_ingest_refusal.is_some())
            .count(),
    )
    .map_err(|_| "M2 refusal count overflow")?;
    Ok(RecordedDrillReport {
        run_profile,
        centre: String::from_utf8_lossy(&centre).into_owned(),
        window_start_ns: start,
        endpoint_class: match endpoint_class {
            EndpointClass::Terminal => "terminal",
            EndpointClass::EnrouteLongRange => "enroute",
        },
        update_interval_ns: policy(endpoint_class).update_interval_ns,
        decision_limit_ns: policy(endpoint_class).max_decision_delay_ns,
        window_end_ns: end,
        capture_start_ns: start,
        warmup_event_count: 0,
        input_ack_count: u64::try_from(receipts.len()).map_err(|_| "input ack count overflow")?,
        warmup_input_ack_count: 0,
        replay_output_ack_count: u64::try_from(replay_consumer_receipts.len())
            .map_err(|_| "replay output ack count overflow")?,
        input_streams,
        stream_counts: StreamCounts::default(),
        region_filter: ZauBoxFilter::default().finish()?,
        normalized_event_count: u64::try_from(events.len()).map_err(|_| "event count overflow")?,
        position_input_count: positions,
        flight_input_count: flights,
        handoff_operation_count: handoffs,
        quarantine_record_count,
        manual_handling_required_count,
        invalid_source_frame_count,
        truncated_source_frame_count,
        skipped_source_bytes,
        source_files_refused,
        consumer_receipts: receipts,
        replay_consumer_receipts,
        cycles,
        per_update_consumer: PerUpdateConsumerReport {
            updates: 0,
            virtual_gap_samples: 0,
            virtual_gap_limit_ns: policy(endpoint_class).update_interval_ns,
            virtual_gap_exceeded: 0,
            max_virtual_gap_ns: None,
            max_monotonic_apply_ack_ns: None,
            decision_limit_ns: policy(endpoint_class).max_decision_delay_ns,
            decision_to_first_output_samples: 0,
            decision_to_first_output_exceeded: 0,
            max_decision_to_first_output_ns: None,
            status: "UNOBSERVABLE_CYCLE_SUMMARY_ONLY",
        },
        replica_cycles,
        model_sha256: oa_sha256::to_hex(&model_sha),
        map_centre_lat_e7,
        map_centre_lon_e7,
        map_range_nm,
        source_state_families_available: [true, true, true, false],
        qualification: "INCOMPLETE_RECORDED_REPLAY_STATE",
        authority_mode: "UNAVAILABLE_NO_LOCAL_FIXTURE",
        m2_attempt_count: m2_cycle_count
            .checked_mul(2)
            .ok_or("M2 attempt count overflow")?,
        m2_refusal_count,
        flip_rollback_attempt_count: 0,
        flip_rollback_complete_count: 0,
        max_retained_snapshot_bytes_per_replica,
    })
}

/// Runs the bounded two-pass H5 replay over a source stream. Capture includes the ten-minute
/// warm-up; only the following fixed hour contributes to the 100 measurement bins.
#[derive(Clone, Copy)]
pub struct RecordedRunOptions<'a> {
    pub worker_executable: &'a Path,
    pub local_authority_fixture: bool,
}

#[derive(Default)]
struct LiveCycleState {
    frames: u64,
    position_inputs: u64,
    flight_inputs: u64,
    handoff_inputs: u64,
    m2_attempted: bool,
    m2_refusal: Option<String>,
    m2_validation_ns: Option<u128>,
    m1_refusal: Option<String>,
    m1_validation_ns: Option<u128>,
    flip_attempted: bool,
    flip_completed: bool,
    selected: Option<Output>,
    decision_ack_ns: Option<u128>,
    rollback_ack_ns: Option<u128>,
    last_generation: Option<u64>,
    last_counts: Option<[u64; 4]>,
    last_digest: Option<String>,
    last_display_rows: Option<u64>,
    output_row_acks: u64,
    max_apply_ack_ns: Option<u128>,
    max_virtual_gap_ns: Option<i64>,
    display_equal: bool,
    initialized: bool,
}

#[derive(Default)]
struct LiveUpdateState {
    updates: u64,
    gap_samples: u64,
    gap_exceeded: u64,
    max_gap_ns: Option<i64>,
    max_apply_ack_ns: Option<u128>,
    decision_samples: u64,
    decision_exceeded: u64,
    max_decision_ns: Option<u128>,
    output_rows_acked: u64,
    last_virtual_ns: Option<i64>,
}

struct LivePairProcessor<'a> {
    consumer: &'a mut Consumer,
    policy: UnitPolicy,
    local_authority_fixture: bool,
    cycles: Vec<LiveCycleState>,
    metrics: LiveUpdateState,
    generation: u64,
    peak_legacy_snapshot_bytes: usize,
    peak_candidate_snapshot_bytes: usize,
}

fn apply_live_snapshot(
    consumer: &mut Consumer,
    generation: &mut u64,
    snapshot: &ReplaySnapshot,
    selected_side: Output,
) -> Result<([u64; 4], String, u64, u128, u64), String> {
    let next_generation = generation
        .checked_add(1)
        .ok_or("H5 state generation overflow")?;
    let transaction = next_generation
        .checked_sub(1)
        .ok_or("H5 state transaction underflow")?;
    let applied = consumer.apply_selected_snapshot(
        transaction,
        next_generation,
        snapshot.view(),
        &snapshot.display_targets,
        selected_side,
    )?;
    *generation = next_generation;
    Ok((applied.0, applied.1, applied.2, applied.3, next_generation))
}

fn checked_frame_gap(previous_ns: Option<i64>, current_ns: i64) -> Result<Option<i64>, String> {
    let Some(previous_ns) = previous_ns else {
        return Ok(None);
    };
    let gap = current_ns
        .checked_sub(previous_ns)
        .ok_or("consumer virtual-time gap overflow")?;
    if gap <= 0 {
        return Err("consumer received duplicate or non-increasing frame time".into());
    }
    Ok(Some(gap))
}

fn exceeds_latency_limit(elapsed_ns: u128, limit_ns: i64) -> bool {
    elapsed_ns > limit_ns as u128
}

impl LivePairProcessor<'_> {
    fn visit(
        &mut self,
        cycle_index: usize,
        legacy: ReplaySnapshot,
        candidate: ReplaySnapshot,
    ) -> Result<(), String> {
        if legacy.observed_time_ns != candidate.observed_time_ns
            || legacy.sequence != candidate.sequence
            || legacy.unit != candidate.unit
            || legacy.side != Side::Legacy
            || candidate.side != Side::Candidate
        {
            return Err("paired runtime frames differ in time, sequence, unit, or side".into());
        }
        let pair_bytes = replay_snapshot_bytes(&legacy)?
            .checked_add(replay_snapshot_bytes(&candidate)?)
            .ok_or("paired frame snapshot byte arithmetic overflow")?;
        if pair_bytes > MAX_H5_PAIRED_FRAME_SNAPSHOT_BYTES {
            return Err("paired current-frame snapshot byte cap exceeded".into());
        }
        self.peak_legacy_snapshot_bytes = self
            .peak_legacy_snapshot_bytes
            .max(replay_snapshot_bytes(&legacy)?);
        self.peak_candidate_snapshot_bytes = self
            .peak_candidate_snapshot_bytes
            .max(replay_snapshot_bytes(&candidate)?);
        let state_slot = self
            .cycles
            .get_mut(cycle_index)
            .ok_or("paired frame cycle index exceeds fixed table")?;
        let mut state = std::mem::take(state_slot);
        state.frames = state
            .frames
            .checked_add(1)
            .ok_or("paired cycle frame count overflow")?;
        state.display_equal &=
            display_targets_equal(&legacy.display_targets, &candidate.display_targets);

        let virtual_ns = legacy.observed_time_ns;
        let mut frame_ack_ns = None;
        let mut selected_frame_applied = false;
        if !state.m2_attempted {
            state.m2_attempted = true;
            let decision_started = Instant::now();
            let unit = legacy.unit;
            if self.local_authority_fixture {
                match prepare_local_fixture_candidate(
                    unit,
                    self.policy,
                    virtual_ns,
                    legacy.view(),
                    candidate.view(),
                    cycle_index,
                ) {
                    Ok(fixture) => {
                        state.m2_validation_ns = Some(fixture.m2_validation_ns);
                        state.m1_validation_ns = Some(fixture.m1_fixture_ns);
                        state.flip_attempted = true;
                        let mut selector = fixture.selector;
                        let (counts, digest, rows, ack_ns, generation) = apply_live_snapshot(
                            self.consumer,
                            &mut self.generation,
                            &candidate,
                            Output::Candidate,
                        )?;
                        let candidate_ack_ns = virtual_ns
                            .checked_add(
                                i64::try_from(ack_ns)
                                    .map_err(|_| "candidate ACK duration exceeds i64")?,
                            )
                            .ok_or("candidate ACK virtual time overflow")?;
                        observe_delivery(
                            &mut selector,
                            self.policy.consumer_id,
                            Output::Candidate,
                            1,
                            candidate_ack_ns,
                        )
                        .map_err(|error| format!("candidate state ACK refused: {error:?}"))?;
                        self.record_rows(rows)?;
                        state.output_row_acks = state
                            .output_row_acks
                            .checked_add(rows)
                            .ok_or("cycle output ACK row count overflow")?;
                        state.max_apply_ack_ns = Some(
                            state
                                .max_apply_ack_ns
                                .map_or(ack_ns, |current| current.max(ack_ns)),
                        );
                        state.last_generation = Some(generation);
                        state.last_counts = Some(counts);
                        state.last_digest = Some(digest);
                        state.last_display_rows = Some(
                            u64::try_from(candidate.display_targets.len())
                                .map_err(|_| "candidate display block count overflow")?,
                        );
                        state.decision_ack_ns = Some(decision_started.elapsed().as_nanos());
                        self.record_decision(state.decision_ack_ns.unwrap_or(0))?;

                        let mut event_material = Vec::with_capacity(40);
                        event_material.extend_from_slice(b"H5 local fixture rollback v1");
                        event_material.extend_from_slice(
                            &u64::try_from(cycle_index)
                                .map_err(|_| "rollback cycle conversion overflow")?
                                .to_le_bytes(),
                        );
                        let rollback_receipt = RollbackReceipt {
                            unit,
                            kind: RollbackKind::ControllerRequest,
                            principal_id: [8; 32],
                            principal_role_id: 1,
                            policy_version: self.policy.policy_version,
                            policy_digest: self.policy.policy_digest,
                            source_epoch_digest: self.policy.source_epoch_digest,
                            evidence_set_handle: self.policy.evidence_set_handle,
                            approval_set_handle: self.policy.approval_set_handle,
                            sequence: 2,
                            legacy_epoch: legacy.source_epoch,
                            candidate_epoch: candidate.source_epoch,
                            legacy_state_version: legacy.state_version,
                            candidate_state_version: candidate.state_version,
                            legacy_sequence: legacy.sequence,
                            candidate_sequence: candidate.sequence,
                            legacy_health: legacy.health,
                            legacy_source_available: true,
                            legacy_health_observed_ns: legacy.observed_time_ns,
                            event_id: oa_sha256::sha256(&event_material),
                            observed_ns: candidate_ack_ns,
                        };
                        rollback(
                            &mut selector,
                            rollback_receipt,
                            candidate_ack_ns,
                            self.policy.max_snapshot_age_ns,
                        )
                        .map_err(|error| format!("local fixture rollback refused: {error:?}"))?;
                        let (counts, digest, rows, ack_ns, generation) = apply_live_snapshot(
                            self.consumer,
                            &mut self.generation,
                            &legacy,
                            Output::Legacy,
                        )?;
                        let rollback_ack_ns = candidate_ack_ns
                            .checked_add(
                                i64::try_from(ack_ns)
                                    .map_err(|_| "rollback ACK duration exceeds i64")?,
                            )
                            .ok_or("rollback ACK virtual time overflow")?;
                        observe_delivery(
                            &mut selector,
                            self.policy.consumer_id,
                            Output::Legacy,
                            2,
                            rollback_ack_ns,
                        )
                        .map_err(|error| format!("rollback state ACK refused: {error:?}"))?;
                        self.record_rows(rows)?;
                        state.output_row_acks = state
                            .output_row_acks
                            .checked_add(rows)
                            .ok_or("cycle output ACK row count overflow")?;
                        state.max_apply_ack_ns = Some(
                            state
                                .max_apply_ack_ns
                                .map_or(ack_ns, |current| current.max(ack_ns)),
                        );
                        state.last_generation = Some(generation);
                        state.last_counts = Some(counts);
                        state.last_digest = Some(digest);
                        state.last_display_rows = Some(
                            u64::try_from(legacy.display_targets.len())
                                .map_err(|_| "rollback display block count overflow")?,
                        );
                        state.rollback_ack_ns = Some(ack_ns);
                        frame_ack_ns = Some(ack_ns);
                        state.flip_completed = true;
                        state.selected = Some(Output::Legacy);
                        selected_frame_applied = true;
                    }
                    Err(error) => {
                        state.m2_refusal = Some(error.clone());
                        if error.starts_with("local fixture M1") {
                            state.m1_refusal = Some(error);
                        }
                        state.selected = Some(Output::Legacy);
                    }
                }
            } else {
                let (refusal, elapsed, selected_side) = m2_attempt_for_runtime_snapshots(
                    unit,
                    self.policy,
                    virtual_ns,
                    legacy.view(),
                    candidate.view(),
                );
                state.m2_refusal = Some(refusal);
                state.m2_validation_ns = Some(elapsed);
                let m1_key = oa_migration::SwapUnitKey {
                    function_id: unit.function_id,
                    scope_id: unit.scope_id,
                    window_start_ns: unit.window_start_ns,
                    window_end_ns: unit.window_end_ns,
                };
                let (m1_refusal, m1_elapsed) =
                    m1_refusal_without_authority(m1_key, unit.window_start_ns);
                state.m1_refusal = Some(m1_refusal);
                state.m1_validation_ns = Some(m1_elapsed);
                state.selected = Some(selected_side);
            }
            if !selected_frame_applied {
                let output = state.selected.unwrap_or(Output::Legacy);
                let snapshot = match output {
                    Output::Legacy => &legacy,
                    Output::Candidate => &candidate,
                };
                let (counts, digest, rows, ack_ns, generation) =
                    apply_live_snapshot(self.consumer, &mut self.generation, snapshot, output)?;
                self.record_rows(rows)?;
                state.output_row_acks = state
                    .output_row_acks
                    .checked_add(rows)
                    .ok_or("cycle output ACK row count overflow")?;
                state.max_apply_ack_ns = Some(
                    state
                        .max_apply_ack_ns
                        .map_or(ack_ns, |current| current.max(ack_ns)),
                );
                state.last_generation = Some(generation);
                state.last_counts = Some(counts);
                state.last_digest = Some(digest);
                state.last_display_rows = Some(
                    u64::try_from(snapshot.display_targets.len())
                        .map_err(|_| "selected display block count overflow")?,
                );
                frame_ack_ns = Some(ack_ns);
                state.decision_ack_ns = Some(decision_started.elapsed().as_nanos());
                self.record_decision(state.decision_ack_ns.unwrap_or(0))?;
                selected_frame_applied = true;
            }
        }
        if !selected_frame_applied {
            let output = state.selected.unwrap_or(Output::Legacy);
            let snapshot = match output {
                Output::Legacy => &legacy,
                Output::Candidate => &candidate,
            };
            let (counts, digest, rows, ack_ns, generation) =
                apply_live_snapshot(self.consumer, &mut self.generation, snapshot, output)?;
            self.record_rows(rows)?;
            state.output_row_acks = state
                .output_row_acks
                .checked_add(rows)
                .ok_or("cycle output ACK row count overflow")?;
            state.max_apply_ack_ns = Some(
                state
                    .max_apply_ack_ns
                    .map_or(ack_ns, |current| current.max(ack_ns)),
            );
            state.last_generation = Some(generation);
            state.last_counts = Some(counts);
            state.last_digest = Some(digest);
            state.last_display_rows = Some(
                u64::try_from(snapshot.display_targets.len())
                    .map_err(|_| "selected display block count overflow")?,
            );
            frame_ack_ns = Some(ack_ns);
        }
        if let Some(gap) =
            self.record_update(virtual_ns, frame_ack_ns.ok_or("frame has no consumer ACK")?)?
        {
            state.max_virtual_gap_ns = Some(
                state
                    .max_virtual_gap_ns
                    .map_or(gap, |current| current.max(gap)),
            );
        }
        state.initialized = true;
        *self
            .cycles
            .get_mut(cycle_index)
            .ok_or("paired frame cycle index exceeds fixed table")? = state;
        Ok(())
    }

    fn record_rows(&mut self, rows: u64) -> Result<(), String> {
        self.metrics.output_rows_acked = self
            .metrics
            .output_rows_acked
            .checked_add(rows)
            .ok_or("consumer applied row count overflow")?;
        Ok(())
    }

    fn record_decision(&mut self, elapsed_ns: u128) -> Result<(), String> {
        self.metrics.decision_samples = self
            .metrics
            .decision_samples
            .checked_add(1)
            .ok_or("decision latency sample count overflow")?;
        self.metrics.max_decision_ns = Some(
            self.metrics
                .max_decision_ns
                .map_or(elapsed_ns, |current| current.max(elapsed_ns)),
        );
        if exceeds_latency_limit(elapsed_ns, self.policy.max_decision_delay_ns) {
            self.metrics.decision_exceeded = self
                .metrics
                .decision_exceeded
                .checked_add(1)
                .ok_or("decision latency exceedance overflow")?;
        }
        Ok(())
    }

    fn record_update(&mut self, virtual_ns: i64, ack_ns: u128) -> Result<Option<i64>, String> {
        self.metrics.updates = self
            .metrics
            .updates
            .checked_add(1)
            .ok_or("consumer update count overflow")?;
        self.metrics.max_apply_ack_ns = Some(
            self.metrics
                .max_apply_ack_ns
                .map_or(ack_ns, |current| current.max(ack_ns)),
        );
        let update_gap = checked_frame_gap(self.metrics.last_virtual_ns, virtual_ns)?;
        if let Some(gap) = update_gap {
            self.metrics.gap_samples = self
                .metrics
                .gap_samples
                .checked_add(1)
                .ok_or("virtual-gap sample count overflow")?;
            self.metrics.max_gap_ns = Some(
                self.metrics
                    .max_gap_ns
                    .map_or(gap, |current| current.max(gap)),
            );
            if exceeds_latency_limit(gap as u128, self.policy.update_interval_ns) {
                self.metrics.gap_exceeded = self
                    .metrics
                    .gap_exceeded
                    .checked_add(1)
                    .ok_or("virtual-gap exceedance count overflow")?;
            }
        }
        self.metrics.last_virtual_ns = Some(virtual_ns);
        Ok(update_gap)
    }
}

fn capture_replay_frame(
    index: usize,
    frame: &RegionalFrame,
    spec: ReplaySpec<'_>,
) -> Result<(usize, ReplaySnapshot), String> {
    let unit = cycle_window(index, spec.start, spec.end)?;
    let snapshot = snapshot_from_runtime_frame(
        frame,
        unit,
        spec.snapshot_policy,
        spec.side,
        spec.source_epoch,
        spec.local_authority_fixture,
    )?;
    Ok((index, snapshot))
}

fn push_replay_frame(
    replay: &mut RegionalReplayAccumulator,
    event: crate::input::NormalizedEvent,
    spec: ReplaySpec<'_>,
) -> Result<Option<(usize, ReplaySnapshot)>, String> {
    let mut captured = None;
    regional_replay_push_observed(replay, event, |index, frame| {
        captured = Some(capture_replay_frame(index, frame, spec)?);
        Ok(())
    })?;
    Ok(captured)
}

fn finish_replay_frames(
    replay: RegionalReplayAccumulator,
    spec: ReplaySpec<'_>,
) -> Result<(Vec<ReplicaCycle>, Option<(usize, ReplaySnapshot)>), String> {
    let mut captured = None;
    let cycles = regional_replay_finish_observed(replay, |index, frame| {
        captured = Some(capture_replay_frame(index, frame, spec)?);
        Ok(())
    })?;
    Ok((cycles, captured))
}

/// One guarded source pass drives two independent RegionalSession runtimes. Each matched frame
/// is immediately offered to M2 and applied to the typed local consumer; only fixed 100-bin
/// aggregates and the current pair of snapshots are retained.
pub fn run_recorded_stream(
    inputs: &[StreamInput],
    centre: [u8; 4],
    endpoint_class: EndpointClass,
    model_bytes: &[u8],
    map_range_nm: f64,
    options: RecordedRunOptions<'_>,
) -> Result<RecordedDrillReport, String> {
    let RecordedRunOptions {
        worker_executable,
        local_authority_fixture,
    } = options;
    if local_authority_fixture
        && inputs
            .iter()
            .any(|input| input.source != RecordingSource::Adsb)
    {
        return Err("local authority fixture accepts ADS-B-only replay inputs".into());
    }
    if H5_CAPTURE_START_NS != 1_795_121_400_000_000_000
        || H5_MEASUREMENT_START_NS != 1_795_122_000_000_000_000
        || H5_MEASUREMENT_END_NS != 1_795_125_600_000_000_000
    {
        return Err("frozen H5 capture/measurement window mismatch".into());
    }
    let run_started = Instant::now();
    let model_sha = oa_sha256::sha256(model_bytes);
    let model = oa_airspace::core::snapshot::decode(model_bytes)
        .map_err(|error| format!("model snapshot decode: {error:?}"))?;
    let (map_centre_lat_e7, map_centre_lon_e7) = model_centre(&model)?;
    drop(model);
    let profile = RegionalRunProfile::recorded_peak_rendered();
    let snapshot_policy = policy(endpoint_class);
    let mut legacy_replay = regional_replay_new_with_warmup(
        model_bytes,
        model_sha,
        map_range_nm,
        H5_CAPTURE_START_NS,
        H5_MEASUREMENT_START_NS,
        H5_MEASUREMENT_END_NS,
        profile,
    )?;
    let mut candidate_replay = regional_replay_new_with_warmup(
        model_bytes,
        model_sha,
        map_range_nm,
        H5_CAPTURE_START_NS,
        H5_MEASUREMENT_START_NS,
        H5_MEASUREMENT_END_NS,
        profile,
    )?;
    let mut consumer = Consumer::spawn(worker_executable)?;
    let mut cycles = Vec::new();
    cycles
        .try_reserve_exact(CYCLES)
        .map_err(|_| "paired H5 cycle table allocation refused")?;
    cycles.extend((0..CYCLES).map(|_| LiveCycleState {
        display_equal: true,
        ..LiveCycleState::default()
    }));
    let mut processor = LivePairProcessor {
        consumer: &mut consumer,
        policy: snapshot_policy,
        local_authority_fixture,
        cycles,
        metrics: LiveUpdateState::default(),
        generation: 0,
        peak_legacy_snapshot_bytes: 0,
        peak_candidate_snapshot_bytes: 0,
    };
    let mut counts = StreamPassCounts {
        normalized_events: 0,
        positions: 0,
        flights: 0,
        handoffs: 0,
        warmup_events: 0,
        input_ack_count: 0,
        warmup_ack_count: 0,
        cycle_input_acks: [0; CYCLES],
        cycle_input_max_ack_ns: [0; CYCLES],
        cycle_positions: [0; CYCLES],
        cycle_flights: [0; CYCLES],
        cycle_handoffs: [0; CYCLES],
    };
    let mut region_filter = ZauBoxFilter::default();
    let legacy_spec = ReplaySpec {
        model_bytes,
        model_sha,
        range_nm: map_range_nm,
        capture_start: H5_CAPTURE_START_NS,
        start: H5_MEASUREMENT_START_NS,
        end: H5_MEASUREMENT_END_NS,
        profile,
        snapshot_policy,
        side: Side::Legacy,
        source_epoch: 1,
        local_authority_fixture,
    };
    let candidate_spec = ReplaySpec {
        side: Side::Candidate,
        source_epoch: 2,
        ..legacy_spec
    };
    let summary = stream_normalized_events(
        inputs,
        Some((H5_CAPTURE_START_NS, H5_MEASUREMENT_END_NS)),
        centre,
        |event, _provenance| {
            if run_started.elapsed() > MAX_RUNTIME {
                return Err("recorded H5 runtime cap exceeded".into());
            }
            counts.normalized_events = counts
                .normalized_events
                .checked_add(1)
                .ok_or("normalized event count overflow")?;
            if counts.normalized_events > profile.max_total_events as u64 {
                return Err("recorded profile normalized event cap exceeded".into());
            }
            if !region_filter.admit(&event)? {
                return Ok(());
            }
            if event.receipt_ns < H5_MEASUREMENT_START_NS {
                counts.warmup_events = counts
                    .warmup_events
                    .checked_add(1)
                    .ok_or("warm-up event count overflow")?;
            } else {
                let index = row_cycle_in_span(
                    event.receipt_ns,
                    H5_MEASUREMENT_START_NS,
                    H5_MEASUREMENT_END_NS,
                )?;
                match (&event.source, &event.kind) {
                    (SourceKind::Surveillance, EventKind::Position { .. }) => {
                        let slot = counts
                            .cycle_positions
                            .get_mut(index)
                            .ok_or("position bin")?;
                        *slot = slot.checked_add(1).ok_or("position count overflow")?;
                    }
                    (SourceKind::Flight, EventKind::Flight(_)) => {
                        let slot = counts.cycle_flights.get_mut(index).ok_or("flight bin")?;
                        *slot = slot.checked_add(1).ok_or("flight count overflow")?;
                        if handoff_event(&event) {
                            let slot = counts.cycle_handoffs.get_mut(index).ok_or("handoff bin")?;
                            *slot = slot.checked_add(1).ok_or("handoff count overflow")?;
                        }
                    }
                    _ => (),
                }
            }
            match (&event.source, &event.kind) {
                (SourceKind::Surveillance, EventKind::Position { .. }) => {
                    counts.positions = counts
                        .positions
                        .checked_add(1)
                        .ok_or("position count overflow")?;
                }
                (SourceKind::Flight, EventKind::Flight(_)) => {
                    counts.flights = counts
                        .flights
                        .checked_add(1)
                        .ok_or("flight count overflow")?;
                    if handoff_event(&event) {
                        counts.handoffs = counts
                            .handoffs
                            .checked_add(1)
                            .ok_or("handoff count overflow")?;
                    }
                }
                _ => (),
            }
            let left = push_replay_frame(&mut legacy_replay, event, legacy_spec)?;
            let right = push_replay_frame(&mut candidate_replay, event.clone(), candidate_spec)?;
            match (left, right) {
                (Some((left_bin, left)), Some((right_bin, right))) if left_bin == right_bin => {
                    processor.visit(left_bin, left, right)?;
                }
                (None, None) => (),
                _ => return Err("independent runtime frame boundaries diverged".into()),
            }
            Ok(())
        },
    )?;
    if summary.normalized_events != counts.normalized_events {
        return Err("stream callback/reader normalized event count mismatch".into());
    }
    let (legacy_cycles, legacy_final) = finish_replay_frames(legacy_replay, legacy_spec)?;
    let (candidate_cycles, candidate_final) =
        finish_replay_frames(candidate_replay, candidate_spec)?;
    match (legacy_final, candidate_final) {
        (Some((left_bin, left)), Some((right_bin, right))) if left_bin == right_bin => {
            processor.visit(left_bin, left, right)?;
        }
        (None, None) => (),
        _ => return Err("independent runtime final frame boundaries diverged".into()),
    }
    if legacy_cycles.len() != CYCLES || candidate_cycles.len() != CYCLES {
        return Err("regional replay cycle aggregate count differs from fixed H5 bins".into());
    }
    processor.consumer.finish()?;
    if run_started.elapsed() > MAX_RUNTIME {
        return Err("recorded H5 runtime cap exceeded after paired replay".into());
    }
    let region_report = region_filter.finish()?;
    let streams = h5_stream_reports(
        &summary,
        H5_MEASUREMENT_START_NS,
        H5_MEASUREMENT_END_NS,
        snapshot_policy.update_interval_ns,
    )?;
    let source_activity = source_coverage(
        &summary.recording.streams,
        RecordingSource::Adsb,
        (H5_CAPTURE_START_NS, H5_MEASUREMENT_END_NS),
        snapshot_policy.update_interval_ns,
    )?;
    let full_local_source = local_authority_fixture
        && source_activity.full_window_covered
        && summary.normalized_events > 0
        && summary.recording.counts.frames > 0
        && summary.recording.counts.reject_frames == 0
        && summary.quarantine_rows == 0
        && summary.manual_handling_required == 0
        && summary.unprocessed_frames == 0;

    let mut cycle_reports = Vec::new();
    cycle_reports
        .try_reserve_exact(CYCLES)
        .map_err(|_| "H5 cycle report allocation refused")?;
    for index in 0..CYCLES {
        let unit = cycle_window(index, H5_MEASUREMENT_START_NS, H5_MEASUREMENT_END_NS)?;
        let state = processor
            .cycles
            .get(index)
            .ok_or("H5 cycle state missing")?;
        let legacy_cycle = legacy_cycles
            .get(index)
            .ok_or("legacy cycle aggregate missing")?;
        let candidate_cycle = candidate_cycles
            .get(index)
            .ok_or("candidate cycle aggregate missing")?;
        let m1_refusal = state.m1_refusal.clone().or_else(|| {
            (!local_authority_fixture).then(|| {
                m1_refusal_without_authority(
                    oa_migration::SwapUnitKey {
                        function_id: unit.function_id,
                        scope_id: unit.scope_id,
                        window_start_ns: unit.window_start_ns,
                        window_end_ns: unit.window_end_ns,
                    },
                    unit.window_start_ns,
                )
                .0
            })
        });
        cycle_reports.push(CycleAttempt {
            index,
            start_ns: unit.window_start_ns,
            end_ns: unit.window_end_ns,
            position_inputs: counts.cycle_positions[index],
            flight_inputs: counts.cycle_flights[index],
            handoff_operations: counts.cycle_handoffs[index],
            consumer_acks: 0,
            consumer_max_ack_ns: None,
            replay_output_max_ack_ns: state.max_apply_ack_ns.unwrap_or(0),
            replay_output_acks: state.output_row_acks,
            selected_state_applied_generation: state.last_generation,
            selected_state_applied_family_counts: state.last_counts,
            selected_display_block_rows: state.last_display_rows,
            display_targets_equal: state.initialized && state.display_equal,
            selected_state_applied_sha256: state.last_digest.clone(),
            decision_to_selected_output_ack_ns: state.decision_ack_ns,
            rollback_output_apply_ack_ns: state.rollback_ack_ns,
            flip_rollback_attempted: state.flip_attempted,
            selected_output: if state.flip_completed {
                "candidate_then_legacy_rollback"
            } else {
                state.selected.map_or("no_frame_no_selection", selected_output_name)
            },
            selector_export_dropped_baseline: legacy_cycle.selector_export_dropped,
            selector_export_dropped_candidate: candidate_cycle.selector_export_dropped,
            replay_projection_equal: legacy_cycle.frames > 0
                && candidate_cycle.frames > 0
                && legacy_cycle.projection_sha256 == candidate_cycle.projection_sha256,
            baseline_projection_sha256: legacy_cycle.projection_sha256.clone(),
            second_projection_sha256: candidate_cycle.projection_sha256.clone(),
            baseline_track_rows: legacy_cycle.track_rows,
            second_track_rows: candidate_cycle.track_rows,
            baseline_flight_views: legacy_cycle.flight_views,
            second_flight_views: candidate_cycle.flight_views,
            baseline_handoff_views: legacy_cycle.handoff_views,
            second_handoff_views: candidate_cycle.handoff_views,
            baseline_active_ca_alerts: legacy_cycle.active_ca_alerts,
            second_active_ca_alerts: candidate_cycle.active_ca_alerts,
            m2_ingest_refusal: state.m2_refusal.clone().or_else(|| {
                (!state.m2_attempted).then(|| "no paired runtime frame in cycle".into())
            }),
            m2_validation_ns: state.m2_validation_ns,
            m1_transition_refusal: m1_refusal,
            m1_validation_ns: state.m1_validation_ns,
            flip_rollback_status: if state.flip_completed {
                "CANDIDATE_APPLIED_ACKED_THEN_ROLLED_BACK_AND_ACKED_LOCAL_FIXTURE"
            } else if state.flip_attempted {
                "INCOMPLETE_FLIP_OR_ROLLBACK"
            } else if state.m2_attempted {
                "M2_REFUSED_DEFAULT_STATE_APPLIED"
            } else {
                "NO_FRAME_NO_M2_ATTEMPT"
            },
            unavailable: if local_authority_fixture {
                "two independent same-implementation RegionalSession runtimes; local synthetic M1 authority is unauthenticated; no FAA legacy/manual control source; ACK/suppression of active alerts remain unknown and cause refusal"
            } else {
                "same-implementation replay sides; selector health, source locator/time basis, complete family coverage, alert acknowledgement/suppression and M1 window authority are unavailable"
            },
            consumer_frame_updates: state.frames,
            consumer_max_virtual_gap_ns: state.max_virtual_gap_ns,
            consumer_max_apply_ack_ns: state.max_apply_ack_ns,
        });
    }
    let update_status = if processor.metrics.updates == 0 {
        "INCOMPLETE_NO_CONSUMER_UPDATES"
    } else if processor.metrics.gap_exceeded > 0 {
        "LIMIT_FAILED_VIRTUAL_UPDATE_GAP"
    } else if processor.metrics.decision_exceeded > 0 {
        "LIMIT_FAILED_DECISION_TO_FIRST_OUTPUT"
    } else if processor.metrics.gap_samples == 0 {
        "INCOMPLETE_NO_VIRTUAL_GAP_SAMPLES"
    } else {
        "MEASURED_WITHIN_DECLARED_LIMITS"
    };
    let cycle_attempts = processor
        .cycles
        .iter()
        .filter(|cycle| cycle.m2_attempted)
        .count();
    let flip_attempts = processor
        .cycles
        .iter()
        .filter(|cycle| cycle.flip_attempted)
        .count();
    let flip_completions = processor
        .cycles
        .iter()
        .filter(|cycle| cycle.flip_completed)
        .count();
    let m2_refusals = processor
        .cycles
        .iter()
        .filter(|cycle| cycle.m2_refusal.is_some())
        .count();
    Ok(RecordedDrillReport {
        run_profile: profile,
        centre: String::from_utf8_lossy(&centre).into_owned(),
        endpoint_class: match endpoint_class {
            EndpointClass::Terminal => "terminal",
            EndpointClass::EnrouteLongRange => "enroute",
        },
        update_interval_ns: snapshot_policy.update_interval_ns,
        decision_limit_ns: snapshot_policy.max_decision_delay_ns,
        window_start_ns: H5_MEASUREMENT_START_NS,
        window_end_ns: H5_MEASUREMENT_END_NS,
        capture_start_ns: H5_CAPTURE_START_NS,
        warmup_event_count: counts.warmup_events,
        input_ack_count: 0,
        warmup_input_ack_count: 0,
        replay_output_ack_count: processor.metrics.output_rows_acked,
        input_streams: streams,
        stream_counts: summary.recording.counts,
        region_filter: region_report,
        normalized_event_count: counts.normalized_events,
        position_input_count: counts.positions,
        flight_input_count: counts.flights,
        handoff_operation_count: counts.handoffs,
        quarantine_record_count: summary.quarantine_rows,
        manual_handling_required_count: summary.manual_handling_required,
        invalid_source_frame_count: 0,
        truncated_source_frame_count: 0,
        skipped_source_bytes: 0,
        source_files_refused: 0,
        consumer_receipts: Vec::new(),
        replay_consumer_receipts: Vec::new(),
        cycles: cycle_reports,
        per_update_consumer: PerUpdateConsumerReport {
            updates: processor.metrics.updates,
            virtual_gap_samples: processor.metrics.gap_samples,
            virtual_gap_limit_ns: snapshot_policy.update_interval_ns,
            virtual_gap_exceeded: processor.metrics.gap_exceeded,
            max_virtual_gap_ns: processor.metrics.max_gap_ns,
            max_monotonic_apply_ack_ns: processor.metrics.max_apply_ack_ns,
            decision_limit_ns: snapshot_policy.max_decision_delay_ns,
            decision_to_first_output_samples: processor.metrics.decision_samples,
            decision_to_first_output_exceeded: processor.metrics.decision_exceeded,
            max_decision_to_first_output_ns: processor.metrics.max_decision_ns,
            status: if full_local_source {
                update_status
            } else {
                "INCOMPLETE_SOURCE_COVERAGE"
            },
        },
        replica_cycles: [legacy_cycles, candidate_cycles],
        model_sha256: oa_sha256::to_hex(&model_sha),
        map_centre_lat_e7,
        map_centre_lon_e7,
        map_range_nm,
        source_state_families_available: [true, true, true, false],
        qualification: if full_local_source
            && m2_refusals == 0
            && flip_completions == CYCLES
            && update_status == "MEASURED_WITHIN_DECLARED_LIMITS"
        {
            "LOCAL_FIXTURE_M2_AND_PER_UPDATE_MECHANISM_ONLY"
        } else {
            "INCOMPLETE_RECORDED_REPLAY_OR_LIMIT_FAILURE"
        },
        authority_mode: if local_authority_fixture {
            "LOCAL_SYNTHETIC_M1_AND_EMPTY_CONTROL_INPUT_FIXTURE_NOT_AUTHENTICATED"
        } else {
            "UNAVAILABLE_NO_LOCAL_FIXTURE"
        },
        m2_attempt_count: u64::try_from(cycle_attempts)
            .map_err(|_| "M2 cycle attempt conversion overflow")?
            .checked_mul(2)
            .ok_or("M2 attempt total overflow")?,
        m2_refusal_count: u64::try_from(m2_refusals)
            .map_err(|_| "M2 refusal conversion overflow")?,
        flip_rollback_attempt_count: u64::try_from(flip_attempts)
            .map_err(|_| "flip/rollback attempt conversion overflow")?,
        flip_rollback_complete_count: u64::try_from(flip_completions)
            .map_err(|_| "flip/rollback completion conversion overflow")?,
        max_retained_snapshot_bytes_per_replica: processor
            .peak_legacy_snapshot_bytes
            .max(processor.peak_candidate_snapshot_bytes),
    })
}

/// Historical cycle-summary implementation retained for diagnostic compatibility.
pub fn run_recorded_stream_cycle_summary(
    inputs: &[StreamInput],
    centre: [u8; 4],
    endpoint_class: EndpointClass,
    model_bytes: &[u8],
    map_range_nm: f64,
    options: RecordedRunOptions<'_>,
) -> Result<RecordedDrillReport, String> {
    let RecordedRunOptions {
        worker_executable,
        local_authority_fixture,
    } = options;
    if local_authority_fixture
        && inputs
            .iter()
            .any(|input| input.source != RecordingSource::Adsb)
    {
        return Err("local authority fixture accepts ADS-B-only replay inputs".into());
    }
    if H5_CAPTURE_START_NS != 1_795_121_400_000_000_000
        || H5_MEASUREMENT_START_NS != 1_795_122_000_000_000_000
        || H5_MEASUREMENT_END_NS != 1_795_125_600_000_000_000
    {
        return Err("frozen H5 capture/measurement window mismatch".into());
    }
    let run_started = Instant::now();
    let model_sha = oa_sha256::sha256(model_bytes);
    let model = oa_airspace::core::snapshot::decode(model_bytes)
        .map_err(|error| format!("model snapshot decode: {error:?}"))?;
    let (map_centre_lat_e7, map_centre_lon_e7) = model_centre(&model)?;
    drop(model);
    // This recorded consumer path must exercise the regional display block builder. Rendering
    // has a predeclared 8,192-update cap; exceeding it refuses the run rather than thinning rows.
    let run_profile = RegionalRunProfile::recorded_peak_rendered();
    let snapshot_policy = policy(endpoint_class);
    let mut input_consumer = Consumer::spawn(worker_executable)?;
    let (baseline, baseline_summary, baseline_counts, baseline_region_filter) = stream_replay_pass(
        StreamReplaySpec {
            inputs,
            centre,
            replay: ReplaySpec {
                model_bytes,
                model_sha,
                range_nm: map_range_nm,
                capture_start: H5_CAPTURE_START_NS,
                start: H5_MEASUREMENT_START_NS,
                end: H5_MEASUREMENT_END_NS,
                profile: run_profile,
                snapshot_policy,
                side: Side::Legacy,
                source_epoch: 1,
                local_authority_fixture,
            },
            run_started,
        },
        Some(&mut input_consumer),
    )?;
    input_consumer.finish()?;
    if run_started.elapsed() > MAX_RUNTIME {
        return Err("recorded H5 runtime cap exceeded after baseline stream".into());
    }
    let (candidate, candidate_summary, candidate_counts, candidate_region_filter) =
        stream_replay_pass(
            StreamReplaySpec {
                inputs,
                centre,
                replay: ReplaySpec {
                    model_bytes,
                    model_sha,
                    range_nm: map_range_nm,
                    capture_start: H5_CAPTURE_START_NS,
                    start: H5_MEASUREMENT_START_NS,
                    end: H5_MEASUREMENT_END_NS,
                    profile: run_profile,
                    snapshot_policy,
                    side: Side::Candidate,
                    source_epoch: 2,
                    local_authority_fixture,
                },
                run_started,
            },
            None,
        )?;
    if baseline_summary != candidate_summary
        || baseline_counts.normalized_events != candidate_counts.normalized_events
        || baseline_counts.positions != candidate_counts.positions
        || baseline_counts.flights != candidate_counts.flights
        || baseline_counts.handoffs != candidate_counts.handoffs
        || baseline_counts.cycle_positions != candidate_counts.cycle_positions
        || baseline_counts.cycle_flights != candidate_counts.cycle_flights
        || baseline_counts.cycle_handoffs != candidate_counts.cycle_handoffs
        || baseline_region_filter != candidate_region_filter
    {
        return Err("independent stream passes differ in normalized coverage".into());
    }
    if run_started.elapsed() > MAX_RUNTIME {
        return Err("recorded H5 runtime cap exceeded after candidate stream".into());
    }
    let max_retained_snapshot_bytes_per_replica = baseline
        .retained_snapshot_bytes
        .max(candidate.retained_snapshot_bytes);
    let snapshot_sets = [baseline.snapshots, candidate.snapshots];
    let replica_cycles = [baseline.cycles, candidate.cycles];
    let mut output_consumer = Consumer::spawn(worker_executable)?;
    let mut output_ack_count = 0u64;
    let mut worker_generation = 0u64;
    let mut cycles = Vec::new();
    cycles
        .try_reserve_exact(CYCLES)
        .map_err(|_| "H5 cycle report allocation refused")?;
    for index in 0..CYCLES {
        if run_started.elapsed() > MAX_RUNTIME {
            return Err("recorded H5 runtime cap exceeded during M2/output pass".into());
        }
        let unit = cycle_window(index, H5_MEASUREMENT_START_NS, H5_MEASUREMENT_END_NS)?;
        let legacy_snapshot = snapshot_sets[0]
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("baseline runtime snapshot missing")?
            .clone();
        let candidate_snapshot = snapshot_sets[1]
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("candidate runtime snapshot missing")?
            .clone();
        let snapshot_now_ns = legacy_snapshot
            .observed_time_ns
            .max(candidate_snapshot.observed_time_ns);
        let mut m2_refusal = None;
        let mut m2_validation_ns = None;
        let mut m1_refusal = None;
        let mut m1_validation_ns = None;
        let mut selected_output = Output::Legacy;
        let mut decision_to_candidate_ack_ns = None;
        let mut rollback_output_ack_ns = None;
        let mut flip_rollback_attempted = false;
        let mut flip_rollback_status = "M2_REFUSED_DEFAULT_STATE_APPLIED";
        let mut cycle_output_acks = 0u64;
        let mut cycle_output_max_ack = 0u128;
        let mut applied_generation = None;
        let mut applied_counts = None;
        let mut applied_display_blocks = None;
        let mut applied_digest = None;
        if local_authority_fixture {
            let decision_started = Instant::now();
            match prepare_local_fixture_candidate(
                unit,
                snapshot_policy,
                snapshot_now_ns,
                legacy_snapshot.view(),
                candidate_snapshot.view(),
                index,
            ) {
                Ok(fixture_candidate) => {
                    let mut selector = fixture_candidate.selector;
                    m2_validation_ns = Some(fixture_candidate.m2_validation_ns);
                    m1_validation_ns = Some(fixture_candidate.m1_fixture_ns);
                    flip_rollback_attempted = true;
                    let mut generation = worker_generation
                        .checked_add(1)
                        .ok_or("selected state generation overflow")?;
                    let candidate_applied = output_consumer.apply_selected_snapshot(
                        generation - 1,
                        generation,
                        candidate_snapshot.view(),
                        &candidate_snapshot.display_targets,
                        Output::Candidate,
                    )?;
                    worker_generation = generation;
                    let candidate_ack_ns = snapshot_now_ns
                        .checked_add(
                            i64::try_from(candidate_applied.3)
                                .map_err(|_| "candidate ACK duration overflow")?,
                        )
                        .ok_or("candidate ACK time overflow")?;
                    observe_delivery(
                        &mut selector,
                        snapshot_policy.consumer_id,
                        Output::Candidate,
                        1,
                        candidate_ack_ns,
                    )
                    .map_err(|error| format!("candidate selected-state ACK refused: {error:?}"))?;
                    decision_to_candidate_ack_ns = Some(decision_started.elapsed().as_nanos());
                    cycle_output_acks = cycle_output_acks
                        .checked_add(candidate_applied.2)
                        .ok_or("candidate ACK row count overflow")?;
                    cycle_output_max_ack = cycle_output_max_ack.max(candidate_applied.3);
                    output_ack_count = output_ack_count
                        .checked_add(candidate_applied.2)
                        .ok_or("selected output ACK count overflow")?;

                    let mut event_id_material = Vec::with_capacity(40);
                    event_id_material.extend_from_slice(b"H5 local fixture rollback v1");
                    event_id_material.extend_from_slice(
                        &u64::try_from(index)
                            .map_err(|_| "rollback cycle conversion overflow")?
                            .to_le_bytes(),
                    );
                    let event_id = oa_sha256::sha256(&event_id_material);
                    let rollback_cause = RollbackReceipt {
                        unit,
                        kind: RollbackKind::ControllerRequest,
                        principal_id: [8; 32],
                        principal_role_id: 1,
                        policy_version: snapshot_policy.policy_version,
                        policy_digest: snapshot_policy.policy_digest,
                        source_epoch_digest: snapshot_policy.source_epoch_digest,
                        evidence_set_handle: snapshot_policy.evidence_set_handle,
                        approval_set_handle: snapshot_policy.approval_set_handle,
                        sequence: 2,
                        legacy_epoch: legacy_snapshot.source_epoch,
                        candidate_epoch: candidate_snapshot.source_epoch,
                        legacy_state_version: legacy_snapshot.state_version,
                        candidate_state_version: candidate_snapshot.state_version,
                        legacy_sequence: legacy_snapshot.sequence,
                        candidate_sequence: candidate_snapshot.sequence,
                        legacy_health: legacy_snapshot.health,
                        legacy_source_available: true,
                        legacy_health_observed_ns: legacy_snapshot.observed_time_ns,
                        event_id,
                        observed_ns: candidate_ack_ns,
                    };
                    rollback(
                        &mut selector,
                        rollback_cause,
                        candidate_ack_ns,
                        snapshot_policy.max_snapshot_age_ns,
                    )
                    .map_err(|error| format!("local fixture rollback refused: {error:?}"))?;

                    generation = worker_generation
                        .checked_add(1)
                        .ok_or("selected state generation overflow")?;
                    let legacy_applied = output_consumer.apply_selected_snapshot(
                        generation - 1,
                        generation,
                        legacy_snapshot.view(),
                        &legacy_snapshot.display_targets,
                        Output::Legacy,
                    )?;
                    applied_display_blocks = Some(
                        u64::try_from(legacy_snapshot.display_targets.len())
                            .map_err(|_| "rollback display block count overflow")?,
                    );
                    worker_generation = generation;
                    let rollback_ack_ns = candidate_ack_ns
                        .checked_add(
                            i64::try_from(legacy_applied.3)
                                .map_err(|_| "rollback ACK duration overflow")?,
                        )
                        .ok_or("rollback ACK time overflow")?;
                    observe_delivery(
                        &mut selector,
                        snapshot_policy.consumer_id,
                        Output::Legacy,
                        2,
                        rollback_ack_ns,
                    )
                    .map_err(|error| format!("rollback selected-state ACK refused: {error:?}"))?;
                    cycle_output_acks = cycle_output_acks
                        .checked_add(legacy_applied.2)
                        .ok_or("rollback ACK row count overflow")?;
                    cycle_output_max_ack = cycle_output_max_ack.max(legacy_applied.3);
                    output_ack_count = output_ack_count
                        .checked_add(legacy_applied.2)
                        .ok_or("selected output ACK count overflow")?;
                    applied_generation = Some(generation);
                    applied_counts = Some(legacy_applied.0);
                    applied_digest = Some(legacy_applied.1);
                    rollback_output_ack_ns = Some(legacy_applied.3);
                    selected_output = selected(&selector);
                    flip_rollback_status =
                        "CANDIDATE_APPLIED_ACKED_THEN_ROLLED_BACK_AND_ACKED_LOCAL_FIXTURE";
                }
                Err(error) => {
                    m2_refusal = Some(error.clone());
                    if error.starts_with("local fixture M1") {
                        m1_refusal = Some(error);
                    }
                }
            }
        } else {
            let (refusal, validation_ns, output) = m2_attempt_for_runtime_snapshots(
                unit,
                snapshot_policy,
                snapshot_now_ns,
                legacy_snapshot.view(),
                candidate_snapshot.view(),
            );
            m2_refusal = Some(refusal);
            m2_validation_ns = Some(validation_ns);
            selected_output = output;
        }
        if applied_generation.is_none() {
            let selected_snapshot = match selected_output {
                Output::Legacy => &legacy_snapshot,
                Output::Candidate => &candidate_snapshot,
            };
            worker_generation = worker_generation
                .checked_add(1)
                .ok_or("selected state generation overflow")?;
            let applied = output_consumer.apply_selected_snapshot(
                worker_generation - 1,
                worker_generation,
                selected_snapshot.view(),
                &selected_snapshot.display_targets,
                selected_output,
            )?;
            applied_display_blocks = Some(
                u64::try_from(selected_snapshot.display_targets.len())
                    .map_err(|_| "selected display block count overflow")?,
            );
            cycle_output_acks = applied.2;
            cycle_output_max_ack = applied.3;
            output_ack_count = output_ack_count
                .checked_add(cycle_output_acks)
                .ok_or("selected output ACK count overflow")?;
            applied_generation = Some(worker_generation);
            applied_counts = Some(applied.0);
            applied_digest = Some(applied.1);
        }
        let left = replica_cycles[0]
            .get(index)
            .ok_or("baseline replay cycle missing")?;
        let right = replica_cycles[1]
            .get(index)
            .ok_or("candidate replay cycle missing")?;
        if !local_authority_fixture {
            let (refusal, validation_ns) = m1_refusal_without_authority(
                oa_migration::SwapUnitKey {
                    function_id: unit.function_id,
                    scope_id: unit.scope_id,
                    window_start_ns: unit.window_start_ns,
                    window_end_ns: unit.window_end_ns,
                },
                unit.window_start_ns,
            );
            m1_refusal = Some(refusal);
            m1_validation_ns = Some(validation_ns);
        }
        let cycle_input_acks = *baseline_counts
            .cycle_input_acks
            .get(index)
            .ok_or("cycle input ACK count index")?;
        let cycle_input_max_ack = *baseline_counts
            .cycle_input_max_ack_ns
            .get(index)
            .ok_or("cycle input ACK latency index")?;
        cycles.push(CycleAttempt {
            index,
            start_ns: unit.window_start_ns,
            end_ns: unit.window_end_ns,
            position_inputs: *baseline_counts
                .cycle_positions
                .get(index)
                .ok_or("cycle position count index")?,
            flight_inputs: *baseline_counts
                .cycle_flights
                .get(index)
                .ok_or("cycle flight count index")?,
            handoff_operations: *baseline_counts
                .cycle_handoffs
                .get(index)
                .ok_or("cycle handoff count index")?,
            consumer_acks: cycle_input_acks,
            consumer_max_ack_ns: (cycle_input_acks > 0).then_some(cycle_input_max_ack),
            replay_output_max_ack_ns: cycle_output_max_ack,
            replay_output_acks: cycle_output_acks,
            selected_state_applied_generation: applied_generation,
            selected_state_applied_family_counts: applied_counts,
            selected_display_block_rows: applied_display_blocks,
            display_targets_equal: display_targets_equal(
                &legacy_snapshot.display_targets,
                &candidate_snapshot.display_targets,
            ),
            selected_state_applied_sha256: applied_digest,
            decision_to_selected_output_ack_ns: decision_to_candidate_ack_ns,
            rollback_output_apply_ack_ns: rollback_output_ack_ns,
            flip_rollback_attempted,
            selected_output: if flip_rollback_attempted {
                "candidate_then_legacy_rollback"
            } else {
                selected_output_name(selected_output)
            },
            selector_export_dropped_baseline: left.selector_export_dropped,
            selector_export_dropped_candidate: right.selector_export_dropped,
            replay_projection_equal: left.frames > 0
                && right.frames > 0
                && left.projection_sha256 == right.projection_sha256,
            baseline_projection_sha256: left.projection_sha256.clone(),
            second_projection_sha256: right.projection_sha256.clone(),
            baseline_track_rows: left.track_rows,
            second_track_rows: right.track_rows,
            baseline_flight_views: left.flight_views,
            second_flight_views: right.flight_views,
            baseline_handoff_views: left.handoff_views,
            second_handoff_views: right.handoff_views,
            baseline_active_ca_alerts: left.active_ca_alerts,
            second_active_ca_alerts: right.active_ca_alerts,
            m2_ingest_refusal: m2_refusal,
            m2_validation_ns,
            m1_transition_refusal: m1_refusal,
            m1_validation_ns,
            flip_rollback_status,
            unavailable: if local_authority_fixture {
                "two same-implementation OpenATCA replay replicas; local synthetic M1 authority fixture is unauthenticated; no FAA legacy/manual control source; active alerts with unknown ACK/suppression refuse M2"
            } else {
                "same-implementation replay sides; selector health, source locator/time basis, complete family coverage, alert acknowledgement/suppression and M1 window authority are unavailable"
            },
            consumer_frame_updates: 0,
            consumer_max_virtual_gap_ns: None,
            consumer_max_apply_ack_ns: None,
        });
    }
    output_consumer.finish()?;
    let input_streams = h5_stream_reports(
        &baseline_summary,
        H5_MEASUREMENT_START_NS,
        H5_MEASUREMENT_END_NS,
        snapshot_policy.update_interval_ns,
    )?;
    let cycle_count = u64::try_from(cycles.len()).map_err(|_| "H5 cycle count overflow")?;
    let m2_refusal_count = u64::try_from(
        cycles
            .iter()
            .filter(|cycle| cycle.m2_ingest_refusal.is_some())
            .count(),
    )
    .map_err(|_| "H5 M2 refusal count overflow")?;
    let flip_rollback_attempt_count = u64::try_from(
        cycles
            .iter()
            .filter(|cycle| cycle.flip_rollback_attempted)
            .count(),
    )
    .map_err(|_| "H5 flip/rollback attempt count overflow")?;
    let flip_rollback_complete_count = u64::try_from(
        cycles
            .iter()
            .filter(|cycle| {
                cycle
                    .flip_rollback_status
                    .starts_with("CANDIDATE_APPLIED_ACKED_THEN_ROLLED_BACK_AND_ACKED")
            })
            .count(),
    )
    .map_err(|_| "H5 flip/rollback completion count overflow")?;
    Ok(RecordedDrillReport {
        run_profile,
        centre: String::from_utf8_lossy(&centre).into_owned(),
        endpoint_class: match endpoint_class {
            EndpointClass::Terminal => "terminal",
            EndpointClass::EnrouteLongRange => "enroute",
        },
        update_interval_ns: snapshot_policy.update_interval_ns,
        decision_limit_ns: snapshot_policy.max_decision_delay_ns,
        window_start_ns: H5_MEASUREMENT_START_NS,
        window_end_ns: H5_MEASUREMENT_END_NS,
        capture_start_ns: H5_CAPTURE_START_NS,
        warmup_event_count: baseline_counts.warmup_events,
        input_ack_count: baseline_counts.input_ack_count,
        warmup_input_ack_count: baseline_counts.warmup_ack_count,
        replay_output_ack_count: output_ack_count,
        input_streams,
        stream_counts: baseline_summary.recording.counts,
        region_filter: baseline_region_filter,
        normalized_event_count: baseline_counts.normalized_events,
        position_input_count: baseline_counts.positions,
        flight_input_count: baseline_counts.flights,
        handoff_operation_count: baseline_counts.handoffs,
        quarantine_record_count: baseline_summary.quarantine_rows,
        manual_handling_required_count: baseline_summary.manual_handling_required,
        invalid_source_frame_count: 0,
        truncated_source_frame_count: 0,
        skipped_source_bytes: 0,
        source_files_refused: 0,
        consumer_receipts: Vec::new(),
        replay_consumer_receipts: Vec::new(),
        cycles,
        per_update_consumer: PerUpdateConsumerReport {
            updates: 0,
            virtual_gap_samples: 0,
            virtual_gap_limit_ns: snapshot_policy.update_interval_ns,
            virtual_gap_exceeded: 0,
            max_virtual_gap_ns: None,
            max_monotonic_apply_ack_ns: None,
            decision_limit_ns: snapshot_policy.max_decision_delay_ns,
            decision_to_first_output_samples: 0,
            decision_to_first_output_exceeded: 0,
            max_decision_to_first_output_ns: None,
            status: "UNOBSERVABLE_CYCLE_SUMMARY_ONLY",
        },
        replica_cycles,
        model_sha256: oa_sha256::to_hex(&model_sha),
        map_centre_lat_e7,
        map_centre_lon_e7,
        map_range_nm,
        source_state_families_available: [true, true, true, false],
        qualification: if local_authority_fixture
            && m2_refusal_count == 0
            && flip_rollback_complete_count == cycle_count
        {
            "LOCAL_FIXTURE_M2_MECHANISM_ONLY"
        } else if local_authority_fixture {
            "LOCAL_FIXTURE_M2_INCOMPLETE_WITH_REFUSALS"
        } else {
            "INCOMPLETE_RECORDED_REPLAY_STATE"
        },
        authority_mode: if local_authority_fixture {
            "LOCAL_SYNTHETIC_M1_AND_EMPTY_CONTROL_INPUT_FIXTURE_NOT_AUTHENTICATED"
        } else {
            "UNAVAILABLE_NO_LOCAL_FIXTURE"
        },
        m2_attempt_count: cycle_count
            .checked_mul(2)
            .ok_or("M2 attempt count overflow")?,
        m2_refusal_count,
        flip_rollback_attempt_count,
        flip_rollback_complete_count,
        max_retained_snapshot_bytes_per_replica,
    })
}

fn event_span(events: &[crate::input::NormalizedEvent]) -> Option<(i64, i64)> {
    let first = events.iter().map(|event| event.receipt_ns).min()?;
    let last = events.iter().map(|event| event.receipt_ns).max()?;
    Some((first, last.checked_add(1)?))
}

fn row_cycle_in_span(time_ns: i64, start: i64, end: i64) -> Result<usize, String> {
    let span = i128::from(end) - i128::from(start);
    if span <= 0 {
        return Err("recorded event span is invalid".into());
    }
    let offset = i128::from(time_ns) - i128::from(start);
    let slot = (offset * CYCLES as i128 / span).clamp(0, (CYCLES - 1) as i128);
    usize::try_from(slot).map_err(|_| "time bin conversion overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ: OA-MIG-414
    #[test]
    fn paired_consumer_gap_and_decision_thresholds_are_inclusive_at_the_limit() {
        assert_eq!(checked_frame_gap(None, 10).expect("first frame"), None);
        assert_eq!(
            checked_frame_gap(Some(10), 4_800_000_010).expect("terminal boundary"),
            Some(4_800_000_000)
        );
        assert!(exceeds_latency_limit(1_000_000_001, 1_000_000_000));
        assert!(!exceeds_latency_limit(1_000_000_000, 1_000_000_000));
        assert!(checked_frame_gap(Some(20), 20).is_err());
        assert!(checked_frame_gap(Some(21), 20).is_err());
    }

    // REQ: OA-MIG-414
    #[test]
    fn regional_traffic_display_block_roundtrips_all_typed_fields() -> Result<(), String> {
        let block = AppliedDisplayTarget {
            lat_e7: 385_000_000,
            lon_e7: -935_000_000,
            alt_ft: 1000,
            addr: 0xabc123,
            r95_bits: 1120403456,
            callsign: *b"TEST123\0",
            track_ddeg: 900,
            gs_dkt: 1234,
            reacq_age_s: 2,
            flags: 3,
            trail_len: 2,
            trail: [(1, 2); oa_display::core::traffic::MAX_TRAIL],
        };
        let encoded = serialize_display_target(Some(&block))?;
        let decoded = parse_display_target(
            encoded
                .strip_prefix("display=")
                .ok_or("display target serialization omitted its field name")?,
        )?;
        assert_eq!(decoded, Some(block));
        Ok(())
    }

    fn spatial_event(
        source: SourceKind,
        address: u32,
        kind: EventKind,
    ) -> crate::input::NormalizedEvent {
        crate::input::NormalizedEvent {
            receipt_ns: H5_CAPTURE_START_NS,
            source_ns: H5_CAPTURE_START_NS,
            sequence: 1,
            source,
            identity: crate::input::Identity {
                flight_gufi: 0,
                aircraft_address: address,
            },
            centre: *b"ZAU1",
            kind,
        }
    }

    fn position(lat: i32, lon: i32) -> EventKind {
        EventKind::Position {
            latitude_e7: lat,
            longitude_e7: lon,
            altitude_ft: Some(1000),
            east_mmps: 0,
            north_mmps: 0,
            vertical_rate_fpm: Some(0),
        }
    }

    #[test]
    fn zau_box_filter_admits_edges_and_only_associated_flight_rows() -> Result<(), String> {
        let mut filter = ZauBoxFilter::default();
        assert!(filter.admit(&spatial_event(
            SourceKind::Surveillance,
            0xabc123,
            position(oa_adsb::ZAU_BOX.lat_min_e7, oa_adsb::ZAU_BOX.lon_min_e7),
        ))?);
        assert!(filter.admit(&spatial_event(
            SourceKind::Flight,
            0xabc123,
            EventKind::Flight(FlightOperation::SetAddress),
        ))?);
        assert!(!filter.admit(&spatial_event(
            SourceKind::Flight,
            0xabc124,
            EventKind::Flight(FlightOperation::SetAddress),
        ))?);
        assert!(!filter.admit(&spatial_event(
            SourceKind::Surveillance,
            0xabc123,
            position(oa_adsb::ZAU_BOX.lat_min_e7 - 1, oa_adsb::ZAU_BOX.lon_min_e7),
        ))?);
        assert!(!filter.admit(&spatial_event(
            SourceKind::Flight,
            0xabc123,
            EventKind::Flight(FlightOperation::SetAddress),
        ))?);
        let report = filter.finish()?;
        assert_eq!(report.position_rows_seen, 2);
        assert_eq!(report.position_rows_in_box, 1);
        assert_eq!(report.position_rows_outside_box, 1);
        assert_eq!(report.flight_rows_seen, 3);
        assert_eq!(report.flight_rows_without_in_box_position, 2);
        assert_eq!(report.admitted_events, 2);
        assert_eq!(report.active_in_box_addresses_at_end, 0);
        assert_eq!(report.distinct_addresses_seen, 1);
        Ok(())
    }

    #[test]
    fn zau_box_filter_does_not_reuse_stale_spatial_association() -> Result<(), String> {
        let mut filter = ZauBoxFilter::default();
        assert!(filter.admit(&spatial_event(
            SourceKind::Surveillance,
            0xabc123,
            position(oa_adsb::ZAU_BOX.lat_min_e7, oa_adsb::ZAU_BOX.lon_min_e7),
        ))?);
        let mut flight = spatial_event(
            SourceKind::Flight,
            0xabc123,
            EventKind::Flight(FlightOperation::SetAddress),
        );
        flight.receipt_ns = H5_CAPTURE_START_NS + ZAU_POSITION_ASSOCIATION_MAX_AGE_NS + 1;
        assert!(!filter.admit(&flight)?);
        let report = filter.finish()?;
        assert_eq!(report.flight_rows_with_stale_in_box_position, 1);
        assert_eq!(report.flight_rows_without_in_box_position, 0);
        assert_eq!(report.active_in_box_addresses_at_end, 0);
        assert_eq!(report.distinct_addresses_seen, 1);
        assert_eq!(report.expired_address_memberships, 1);
        Ok(())
    }

    #[test]
    fn zau_box_filter_refuses_unbounded_address_growth() -> Result<(), String> {
        let mut filter = ZauBoxFilter::default();
        for address in 1..=MAX_ZAU_BOX_ADDRESSES as u32 {
            assert!(filter.admit(&spatial_event(
                SourceKind::Surveillance,
                address,
                position(oa_adsb::ZAU_BOX.lat_min_e7, oa_adsb::ZAU_BOX.lon_min_e7),
            ))?);
        }
        assert!(filter
            .admit(&spatial_event(
                SourceKind::Surveillance,
                MAX_ZAU_BOX_ADDRESSES as u32 + 1,
                position(oa_adsb::ZAU_BOX.lat_min_e7, oa_adsb::ZAU_BOX.lon_min_e7),
            ))
            .is_err());
        Ok(())
    }

    // REQ: OA-MIG-414
    #[test]
    fn selected_snapshot_rows_apply_to_typed_worker_tables_with_the_same_digest(
    ) -> Result<(), String> {
        let track = TrackState {
            key: BusinessKey {
                entity_key: [0x11; 32],
                key_status: FieldStatus::Unknown,
                family: EntityFamily::Track,
                ordinal: 7,
            },
            source_identity: SourceIdentity {
                stream: 0,
                decoded_offset: 0,
                row: 0,
                operation_index: 0,
            },
            time_basis_id: 0,
            source_time_ns: 0,
            observed_time_ns: H5_MEASUREMENT_START_NS,
            status: FieldStatus::Unknown,
            lat_e7: 385_000_000,
            lon_e7: -935_000_000,
            altitude_ft: 1000,
            speed_dkt: 0,
            track_ddeg: 0,
            flags: 0,
            field_mask: 0,
            projection_digest: [0x22; 32],
        };
        let snapshot = Snapshot {
            unit: SwapUnitKey {
                function_id: 1,
                scope_id: 1,
                window_start_ns: 1,
                window_end_ns: 2,
            },
            policy: policy(EndpointClass::Terminal),
            evidence: EvidenceKind::Recorded,
            side: Side::Legacy,
            source_epoch: 1,
            state_version: 1,
            sequence: 1,
            source_time_ns: 0,
            observed_time_ns: H5_MEASUREMENT_START_NS,
            health: Health::Unknown,
            track_coverage: FamilyCoverage::Partial,
            flight_coverage: FamilyCoverage::Partial,
            handoff_coverage: FamilyCoverage::Partial,
            alert_coverage: FamilyCoverage::Partial,
            tracks: std::array::from_fn(|index| (index == 0).then_some(track)),
            flights: std::array::from_fn(|_| None),
            handoffs: std::array::from_fn(|_| None),
            alerts: std::array::from_fn(|_| None),
        };
        let batch = snapshot_output_payloads(snapshot.view(), &[], Output::Legacy)?;
        assert_eq!(batch.family_counts, [1, 0, 0, 0]);
        let row = batch.rows.first().ok_or("selected state row missing")?;
        let mut pending = PendingAppliedState::new(0, 1, "Legacy", [1, 0, 0, 0])?;
        pending.push(1, &oa_sha256::to_hex(&oa_sha256::sha256(row)), row)?;
        let mut applied = AppliedState::default();
        let digest = pending.apply(&mut applied, 0, 1, [1, 0, 0, 0], &batch.sha256)?;
        assert_eq!(digest, batch.sha256);
        assert_eq!(applied.digest, batch.sha256);
        assert_eq!(applied.generation, 1);
        let [track_table, _, _, _] = &applied.tables;
        assert_eq!(track_table.len(), 1);
        assert!(matches!(
            track_table.first(),
            Some(AppliedStateRow::Track {
                lat_e7: 385_000_000,
                lon_e7: -935_000_000,
                altitude_ft: 1000,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn unavailable_recorded_snapshot_is_refused_by_m2() {
        let p = policy(EndpointClass::Terminal);
        let u = SwapUnitKey {
            function_id: 1,
            scope_id: 1,
            window_start_ns: 10,
            window_end_ns: 20,
        };
        assert_eq!(m2_refusal_for_missing_source(u, p, 10).0, "Unhealthy");
        let snapshot = unavailable_snapshot(u, p, Side::Legacy, 10);
        assert_eq!(snapshot.track_coverage, FamilyCoverage::Missing);
        assert_eq!(snapshot.alert_coverage, FamilyCoverage::Missing);
    }

    #[test]
    fn local_authority_fixture_runs_candidate_select_and_acknowledged_rollback(
    ) -> Result<(), String> {
        let policy = policy(EndpointClass::Terminal);
        let unit = SwapUnitKey {
            function_id: 1,
            scope_id: 1,
            window_start_ns: 1_000_000_000,
            window_end_ns: 37_000_000_000,
        };
        let now_ns = unit.window_start_ns + 1_000;
        let mut legacy = unavailable_snapshot(unit, policy, Side::Legacy, now_ns);
        let mut candidate = unavailable_snapshot(unit, policy, Side::Candidate, now_ns);
        for snapshot in [&mut legacy, &mut candidate] {
            snapshot.health = Health::Healthy;
            snapshot.track_coverage = FamilyCoverage::CompleteEmpty;
            snapshot.flight_coverage = FamilyCoverage::CompleteEmpty;
            snapshot.handoff_coverage = FamilyCoverage::CompleteEmpty;
            snapshot.alert_coverage = FamilyCoverage::CompleteEmpty;
        }
        let mut selector = prepare_local_fixture_candidate(
            unit,
            policy,
            now_ns,
            legacy.view(),
            candidate.view(),
            0,
        )?
        .selector;
        assert_eq!(selected(&selector), Output::Candidate);
        let candidate_ack_ns = now_ns + 1;
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Candidate,
            1,
            candidate_ack_ns,
        )
        .map_err(|error| format!("fixture candidate ACK: {error:?}"))?;
        let rollback_cause = RollbackReceipt {
            unit,
            kind: RollbackKind::ControllerRequest,
            principal_id: [8; 32],
            principal_role_id: 1,
            policy_version: policy.policy_version,
            policy_digest: policy.policy_digest,
            source_epoch_digest: policy.source_epoch_digest,
            evidence_set_handle: policy.evidence_set_handle,
            approval_set_handle: policy.approval_set_handle,
            sequence: 2,
            legacy_epoch: legacy.source_epoch,
            candidate_epoch: candidate.source_epoch,
            legacy_state_version: legacy.state_version,
            candidate_state_version: candidate.state_version,
            legacy_sequence: legacy.sequence,
            candidate_sequence: candidate.sequence,
            legacy_health: legacy.health,
            legacy_source_available: true,
            legacy_health_observed_ns: legacy.observed_time_ns,
            event_id: [9; 32],
            observed_ns: candidate_ack_ns,
        };
        rollback(
            &mut selector,
            rollback_cause,
            candidate_ack_ns,
            policy.max_snapshot_age_ns,
        )
        .map_err(|error| format!("fixture rollback: {error:?}"))?;
        assert_eq!(selected(&selector), Output::Legacy);
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Legacy,
            2,
            candidate_ack_ns + 1,
        )
        .map_err(|error| format!("fixture rollback ACK: {error:?}"))?;
        Ok(())
    }

    #[test]
    fn event_time_bins_are_bounded_and_include_last_time() {
        let _events = [
            crate::input::NormalizedEvent {
                receipt_ns: 0,
                source_ns: 0,
                sequence: 1,
                source: SourceKind::Surveillance,
                identity: crate::input::Identity {
                    flight_gufi: 0,
                    aircraft_address: 1,
                },
                centre: *b"ZAU1",
                kind: EventKind::Position {
                    latitude_e7: 1,
                    longitude_e7: 2,
                    altitude_ft: Some(30000),
                    east_mmps: 0,
                    north_mmps: 0,
                    vertical_rate_fpm: Some(0),
                },
            },
            crate::input::NormalizedEvent {
                receipt_ns: 100,
                source_ns: 100,
                sequence: 2,
                source: SourceKind::Surveillance,
                identity: crate::input::Identity {
                    flight_gufi: 0,
                    aircraft_address: 2,
                },
                centre: *b"ZAU1",
                kind: EventKind::Position {
                    latitude_e7: 1,
                    longitude_e7: 2,
                    altitude_ft: Some(30000),
                    east_mmps: 0,
                    north_mmps: 0,
                    vertical_rate_fpm: Some(0),
                },
            },
        ];
        assert!(matches!(row_cycle_in_span(0, 0, 101), Ok(0)));
        assert!(matches!(
            row_cycle_in_span(100, 0, 101),
            Ok(value) if value == CYCLES - 1
        ));
    }

    #[test]
    fn warmup_is_ingested_but_excluded_from_measurement_bins() {
        let capture_start = 1_795_121_400_000_000_000;
        let measurement_start = 1_795_122_000_000_000_000;
        let measurement_end = 1_795_125_600_000_000_000;
        assert_eq!(
            replay_measurement_cycle(
                capture_start,
                capture_start,
                measurement_start,
                measurement_end
            ),
            Ok(None)
        );
        assert_eq!(
            replay_measurement_cycle(
                measurement_start - 1,
                capture_start,
                measurement_start,
                measurement_end
            ),
            Ok(None)
        );
        assert_eq!(
            replay_measurement_cycle(
                measurement_start,
                capture_start,
                measurement_start,
                measurement_end
            ),
            Ok(Some(0))
        );
        assert_eq!(
            replay_measurement_cycle(
                measurement_end - 1,
                capture_start,
                measurement_start,
                measurement_end
            ),
            Ok(Some(CYCLES - 1))
        );
        assert!(replay_measurement_cycle(
            measurement_end,
            capture_start,
            measurement_start,
            measurement_end
        )
        .is_err());
        assert!(replay_measurement_cycle(
            capture_start - 1,
            capture_start,
            measurement_start,
            measurement_end
        )
        .is_err());
    }
}

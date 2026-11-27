//! Explicit offline hold annotations for the regional operator API.
//!
//! The fields follow FAA JO 7110.65 §4-6-4 holding-instruction semantics and §13-1-7's
//! Hold View annotation use. This is an OpenATCA value model, not an FAA wire message, facility
//! directive, authenticated clearance, or holding-pattern trajectory.

use oa_legacy_traj::core::trajectory::{FlightKey, HoldPhaseSnapshot, HoldPhaseState};

use crate::input::RouteFixInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HoldTurn {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldLegUnit {
    Miles,
    Minutes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldLegLength {
    pub value: u16,
    pub unit: HoldLegUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HoldEntryProcedure {
    Direct,
    Parallel,
    Teardrop,
}

/// The course used to classify an observed fix crossing. Ground course is accepted only for a
/// caller-declared synthetic no-wind stream; live/unknown wind or heading semantics are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HoldEntryCourseBasis {
    SyntheticNoWindCourse,
    WindOrHeadingUnknown,
}

/// One accepted surveillance report needed for causal entry inference. `source_epoch` identifies
/// the producer session; reports from different epochs are never joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldEntryReport {
    pub key: FlightKey,
    pub source_epoch: u64,
    pub sequence: u64,
    pub source_ns: i64,
    pub receipt_ns: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub r95_mm: u32,
}

/// Evidence that a pair of consecutive reports passed within the configured model capture radius.
/// `crossing_ns` is an interpolated closest-approach time, while `available_at_ns` is the later
/// report receipt. Consumers must not use the evidence at cutoffs before it became available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldEntryEvidence {
    pub key: FlightKey,
    pub source_epoch: u64,
    pub previous_sequence: u64,
    pub current_sequence: u64,
    pub crossing_ns: i64,
    pub available_at_ns: i64,
    pub crossing_lat_e7: i32,
    pub crossing_lon_e7: i32,
    pub course_true_cdeg: u16,
    pub closest_distance_mm: u32,
    pub combined_r95_mm: u32,
    pub procedure: HoldEntryProcedure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldEntryInferenceError {
    UnavailableCourseBasis,
    InvalidInput,
    IdentityOrSequenceMismatch,
    ScanGapExceeded,
    ClosestPointNotBetweenReports,
    OutsideCaptureRadius,
    UncertaintyExceedsCapture,
    AmbiguousSector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldEntryInferenceConfig {
    pub fix_lat_e7: i32,
    pub fix_lon_e7: i32,
    pub inbound_true_course_cdeg: u16,
    pub turn: HoldTurn,
    pub max_scan_gap_ns: i64,
    pub capture_radius_mm: u32,
    pub sector_ambiguity_cdeg: u16,
    pub course_basis: HoldEntryCourseBasis,
}

/// Infer entry geometry from two accepted reports in the explicit synthetic no-wind model.
/// This computes a model closest approach, not proof that an aircraft crossed the physical fix.
pub fn infer_hold_entry(
    previous: HoldEntryReport,
    current: HoldEntryReport,
    config: HoldEntryInferenceConfig,
) -> Result<HoldEntryEvidence, HoldEntryInferenceError> {
    let HoldEntryInferenceConfig {
        fix_lat_e7,
        fix_lon_e7,
        inbound_true_course_cdeg,
        turn,
        max_scan_gap_ns,
        capture_radius_mm,
        sector_ambiguity_cdeg,
        course_basis,
    } = config;
    if course_basis != HoldEntryCourseBasis::SyntheticNoWindCourse {
        return Err(HoldEntryInferenceError::UnavailableCourseBasis);
    }
    if previous.key != current.key
        || previous.source_epoch == 0
        || previous.source_epoch != current.source_epoch
        || previous.sequence == 0
        || current.sequence <= previous.sequence
    {
        return Err(HoldEntryInferenceError::IdentityOrSequenceMismatch);
    }
    if inbound_true_course_cdeg >= 36_000
        || !(1..=60_000_000_000).contains(&max_scan_gap_ns)
        || !(1..=1_000_000).contains(&capture_radius_mm)
        || sector_ambiguity_cdeg > 1_000
        || !(-900_000_000..=900_000_000).contains(&fix_lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&fix_lon_e7)
        || previous.source_ns <= 0
        || current.source_ns <= previous.source_ns
        || previous.receipt_ns < previous.source_ns
        || current.receipt_ns < current.source_ns
        || current.receipt_ns <= previous.receipt_ns
        || previous.r95_mm == 0
        || current.r95_mm == 0
        || previous.r95_mm > 1_000_000
        || current.r95_mm > 1_000_000
        || !(-900_000_000..=900_000_000).contains(&previous.lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&previous.lon_e7)
        || !(-900_000_000..=900_000_000).contains(&current.lat_e7)
        || !(-1_800_000_000..=1_800_000_000).contains(&current.lon_e7)
    {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let source_gap = current
        .source_ns
        .checked_sub(previous.source_ns)
        .ok_or(HoldEntryInferenceError::InvalidInput)?;
    let receipt_gap = current
        .receipt_ns
        .checked_sub(previous.receipt_ns)
        .ok_or(HoldEntryInferenceError::InvalidInput)?;
    if source_gap > max_scan_gap_ns || receipt_gap > max_scan_gap_ns {
        return Err(HoldEntryInferenceError::ScanGapExceeded);
    }

    const METRES_PER_DEGREE: f64 = 111_194.926_644_558_74;
    let origin_lat = f64::from(fix_lat_e7) / 10_000_000.0;
    let origin_lon = f64::from(fix_lon_e7) / 10_000_000.0;
    let cos_lat = oa_math::cos(origin_lat.to_radians());
    if !cos_lat.is_finite() || cos_lat.abs() < 1.0e-6 {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let local = |report: HoldEntryReport| {
        let lat = f64::from(report.lat_e7) / 10_000_000.0;
        let lon = f64::from(report.lon_e7) / 10_000_000.0;
        (
            (lon - origin_lon) * cos_lat * METRES_PER_DEGREE,
            (lat - origin_lat) * METRES_PER_DEGREE,
        )
    };
    let (x0, y0) = local(previous);
    let (x1, y1) = local(current);
    let dx = x1 - x0;
    let dy = y1 - y0;
    let segment2 = dx * dx + dy * dy;
    if !segment2.is_finite() || segment2 <= 0.0 || segment2 > 20_000.0_f64.powi(2) {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let unbounded_fraction = -(x0 * dx + y0 * dy) / segment2;
    if !unbounded_fraction.is_finite() || !(0.0..1.0).contains(&unbounded_fraction) {
        return Err(HoldEntryInferenceError::ClosestPointNotBetweenReports);
    }
    let fraction = unbounded_fraction.clamp(0.0, 1.0);
    let closest_x = x0 + fraction * dx;
    let closest_y = y0 + fraction * dy;
    let distance = oa_math::hypot(closest_x, closest_y) * 1_000.0;
    let uncertainty = previous.r95_mm.saturating_add(current.r95_mm);
    if !distance.is_finite() || distance > f64::from(capture_radius_mm) {
        return Err(HoldEntryInferenceError::OutsideCaptureRadius);
    }
    if distance + f64::from(uncertainty) > f64::from(capture_radius_mm) {
        return Err(HoldEntryInferenceError::UncertaintyExceedsCapture);
    }
    let crossing_offset = oa_ieee::round(fraction * source_gap as f64 / 1_000_000.0) * 1_000_000.0;
    if !crossing_offset.is_finite() || crossing_offset < 0.0 || crossing_offset > source_gap as f64
    {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let crossing_ns = previous
        .source_ns
        .checked_add(crossing_offset as i64)
        .ok_or(HoldEntryInferenceError::InvalidInput)?;
    let crossing_lat =
        (f64::from(fix_lat_e7) / 10_000_000.0 + closest_y / METRES_PER_DEGREE) * 10_000_000.0;
    let crossing_lon = (f64::from(fix_lon_e7) / 10_000_000.0
        + closest_x / (METRES_PER_DEGREE * cos_lat))
        * 10_000_000.0;
    if !crossing_lat.is_finite() || !crossing_lon.is_finite() {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let crossing_lat_e7 = oa_ieee::round(crossing_lat) as i32;
    let crossing_lon_e7 = oa_ieee::round(crossing_lon) as i32;
    let course = oa_math::rem_euclid(oa_math::atan2(dx, dy).to_degrees(), 360.0) * 100.0;
    if !course.is_finite() {
        return Err(HoldEntryInferenceError::InvalidInput);
    }
    let course_cdeg = oa_ieee::round(course) as i64;
    let inbound = i64::from(inbound_true_course_cdeg);
    let signed = (course_cdeg - inbound + 54_000).rem_euclid(36_000) - 18_000;
    // Figure 5-3-4 supplies the 180/110/70 degree sectors. The ±5° boundary guard is a
    // conservative model rule, not an FAA-prescribed uncertainty interval.
    let (direct_edge, parallel_edge, teardrop_edge) = match turn {
        HoldTurn::Right => (0_i64, -18_000_i64, 7_000_i64),
        HoldTurn::Left => (0_i64, 18_000_i64, -7_000_i64),
    };
    let near = |a: i64, b: i64| {
        let delta = (a - b + 54_000).rem_euclid(36_000) - 18_000;
        delta.unsigned_abs() <= u64::from(sector_ambiguity_cdeg)
    };
    if near(signed, direct_edge) || near(signed, parallel_edge) || near(signed, teardrop_edge) {
        return Err(HoldEntryInferenceError::AmbiguousSector);
    }
    let procedure = match turn {
        HoldTurn::Right if (-18_000..0).contains(&signed) => HoldEntryProcedure::Direct,
        HoldTurn::Right if (0..7_000).contains(&signed) => HoldEntryProcedure::Teardrop,
        HoldTurn::Right => HoldEntryProcedure::Parallel,
        HoldTurn::Left if (0..18_000).contains(&signed) => HoldEntryProcedure::Direct,
        HoldTurn::Left if (-7_000..0).contains(&signed) => HoldEntryProcedure::Teardrop,
        HoldTurn::Left => HoldEntryProcedure::Parallel,
    };
    let closest_distance_mm = oa_ieee::round(distance) as u32;
    Ok(HoldEntryEvidence {
        key: current.key,
        source_epoch: current.source_epoch,
        previous_sequence: previous.sequence,
        current_sequence: current.sequence,
        crossing_ns,
        available_at_ns: current.receipt_ns,
        crossing_lat_e7,
        crossing_lon_e7,
        course_true_cdeg: course_cdeg.rem_euclid(36_000) as u16,
        closest_distance_mm,
        combined_r95_mm: uncertainty,
        procedure,
    })
}

/// Explicit engineering parameters for the first direct-entry prediction slice. The entry time
/// must name the exact admitted track state; all reconciliation bounds are caller selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct HoldPredictionIntent {
    pub entry_procedure: HoldEntryProcedure,
    /// Exact admitted state for Direct. Zero requests report-inferred entry for Parallel/Teardrop.
    pub entry_state_ns: i64,
    /// Required for inferred entries; there is no operational wind/heading fallback.
    pub course_basis: HoldEntryCourseBasis,
    /// Caller-selected maximum closest-approach radius for the inferred fix crossing.
    pub entry_capture_radius_mm: u32,
    pub inbound_true_course_cdeg: u16,
    pub leg_duration_ms: u32,
    pub turn_rate_cdeg_s: u16,
    pub max_scan_gap_ns: i64,
    pub max_position_error_mm: u32,
    pub max_heading_error_cdeg: u16,
    pub max_speed_error_mmps: u32,
}
const _: () = assert!(core::mem::size_of::<HoldPredictionIntent>() == 64);
const _: () = assert!(core::mem::size_of::<Option<HoldPredictionIntent>>() == 64);

/// Explicit caller-supplied holding details, keyed by the complete flight identity.
/// `efc_utc_minute: None` is blank; `Some(0)` is 00:00 UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldInstruction {
    pub flight_key: FlightKey,
    pub fix: RouteFixInput,
    /// Bearing degrees from the holding fix describing the assigned holding direction.
    pub direction_from_fix_deg: u16,
    /// Explicit inbound course/radial in degrees; this model does not infer it from route geometry.
    pub inbound_course_deg: u16,
    /// Optional turn/leg details remain absent when not specified by the caller.
    pub turn: Option<HoldTurn>,
    pub leg: Option<HoldLegLength>,
    /// Minutes after 00:00 UTC, preserving blank separately from midnight.
    pub efc_utc_minute: Option<u16>,
    /// None keeps the annotation visible while hold forecasting remains unavailable.
    pub prediction: Option<HoldPredictionIntent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Typed project annotation request. Revisions increase per full flight key within the supplied
/// runtime-issued lifecycle generation; a retired key receives a new generation if it returns.
pub enum HoldUpdate {
    Assign {
        lifecycle_generation: u64,
        revision: u64,
        instruction: HoldInstruction,
    },
    Amend {
        lifecycle_generation: u64,
        revision: u64,
        instruction: HoldInstruction,
    },
    Cancel {
        lifecycle_generation: u64,
        revision: u64,
        flight_key: FlightKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldViewRow {
    /// Index in the owning HoldStore for bounded cached-profile access during this scan.
    pub(crate) store_index: usize,
    pub instruction: HoldInstruction,
    /// Receipt time of the explicit project API update, not a pilot/ATC time.
    pub updated_receipt_ns: i64,
    /// Monotonic project-API revision accepted for this flight key.
    pub revision: u64,
    pub lifecycle_generation: u64,
    pub phase_state: Option<HoldPhaseState>,
    pub phase_snapshot: Option<HoldPhaseSnapshot>,
}

/// Why a trajectory query was suppressed rather than forecasting along the filed route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldProbeUnavailableReason {
    ActiveHoldPatternNotModeledBySharedPredictor,
    TrackStateNotReconciled,
    MissingAcceptedEntry,
    UnsupportedEntryProcedure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairHoldProbeUnavailable {
    pub first: FlightKey,
    pub second: FlightKey,
    pub reason: HoldProbeUnavailableReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuaHoldProbeUnavailable {
    pub flight: FlightKey,
    pub boundary_index: usize,
    pub reason: HoldProbeUnavailableReason,
}

/// Startup-sized SoA storage for explicit annotations. Query loops inspect only the key column;
/// rows are assembled only when the caller asks for a frame view.
pub(crate) struct HoldStore {
    capacity: usize,
    keys: Vec<FlightKey>,
    fixes: Vec<RouteFixInput>,
    directions: Vec<u16>,
    courses: Vec<u16>,
    turns: Vec<Option<HoldTurn>>,
    legs: Vec<Option<HoldLegLength>>,
    predictions: Vec<Option<HoldPredictionIntent>>,
    phase_entry_ns: Vec<Option<i64>>,
    phase_last_ns: Vec<Option<i64>>,
    phase_speed_mmps: Vec<Option<u32>>,
    phase_models: Vec<Option<HoldPhaseState>>,
    efc_minutes: Vec<Option<u16>>,
    updated_receipt_ns: Vec<i64>,
    revisions: Vec<u64>,
    lifecycle_generations: Vec<u64>,
    // One startup-allocated 240-point buffer per hold row. At the 8,192-flight offline
    // profile ceiling, route points reserve 8,192 × 240 × 60 = 117,964,800 bytes (112.5 MiB).
    profile_points: Vec<oa_legacy_traj::core::trajectory::RoutePoint>,
    profile_states: Vec<Option<HoldPhaseState>>,
    profile_configs: Vec<Option<HoldPredictionIntent>>,
    profile_revisions: Vec<u64>,
    profile_scan_sequences: Vec<u64>,
}

/// Bounded SoA high-water marks outlive cancelled payloads within a flight lifecycle. Entries
/// are reclaimed only after the lifecycle store invalidates that generation on retirement.
pub(crate) struct HoldRevisions {
    capacity: usize,
    keys: Vec<FlightKey>,
    generations: Vec<u64>,
    revisions: Vec<u64>,
}

impl HoldRevisions {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            keys: Vec::with_capacity(capacity),
            generations: Vec::with_capacity(capacity),
            revisions: Vec::with_capacity(capacity),
        }
    }

    fn position(&self, key: FlightKey, generation: u64) -> Option<usize> {
        self.keys
            .iter()
            .zip(&self.generations)
            .position(|(&candidate, &candidate_generation)| {
                candidate == key && candidate_generation == generation
            })
    }

    pub(crate) fn validate(
        &self,
        key: FlightKey,
        generation: u64,
        revision: u64,
    ) -> Result<(), &'static str> {
        let len = self.keys.len();
        if self.generations.len() != len || self.revisions.len() != len {
            return Err("hold revision SoA invariant failed");
        }
        if self.position(key, generation).is_some_and(|index| {
            self.revisions
                .get(index)
                .is_some_and(|&last| revision <= last)
        }) {
            return Err("hold update revision is stale or out of order for this full flight key");
        }
        if self.position(key, generation).is_none() && len >= self.capacity {
            return Err("hold revision ledger capacity exhausted; update refused");
        }
        Ok(())
    }

    pub(crate) fn last_revision(&self, key: FlightKey, generation: u64) -> Option<u64> {
        self.position(key, generation)
            .and_then(|index| self.revisions.get(index).copied())
    }

    pub(crate) fn record(
        &mut self,
        key: FlightKey,
        generation: u64,
        revision: u64,
    ) -> Result<(), &'static str> {
        if let Some(index) = self.position(key, generation) {
            let Some(last) = self.revisions.get_mut(index) else {
                return Err("hold revision SoA invariant failed");
            };
            *last = revision;
            return Ok(());
        }
        if self.keys.len() >= self.capacity {
            return Err("hold revision ledger capacity exhausted; update refused");
        }
        let len = self.keys.len();
        if self.generations.len() != len || self.revisions.len() != len {
            return Err("hold revision SoA invariant failed");
        }
        self.keys.push(key);
        self.generations.push(generation);
        self.revisions.push(revision);
        Ok(())
    }

    pub(crate) fn remove_keys(&mut self, retired: &[FlightKey]) -> Result<(), &'static str> {
        if self.keys.len() > self.capacity
            || self.keys.len() != self.generations.len()
            || self.keys.len() != self.revisions.len()
        {
            return Err("hold revision SoA invariant failed during retirement");
        }
        if retired.len() > self.capacity {
            return Err("retired hold revision key count exceeded capacity");
        }
        if retired.is_empty() {
            return Ok(());
        }
        let mut write = 0usize;
        let len = self.keys.len();
        for read in 0..len {
            let key = *self
                .keys
                .get(read)
                .ok_or("hold revision retirement key invariant failed")?;
            if retired
                .binary_search_by(|candidate| compare_flight_keys(candidate, &key))
                .is_ok()
            {
                continue;
            }
            if write != read {
                *self
                    .keys
                    .get_mut(write)
                    .ok_or("hold revision retirement destination invariant failed")? = key;
                *self
                    .generations
                    .get_mut(write)
                    .ok_or("hold revision generation destination invariant failed")? = *self
                    .generations
                    .get(read)
                    .ok_or("hold revision generation invariant failed")?;
                *self
                    .revisions
                    .get_mut(write)
                    .ok_or("hold revision value destination invariant failed")? = *self
                    .revisions
                    .get(read)
                    .ok_or("hold revision value invariant failed")?;
            }
            write += 1;
        }
        self.keys.truncate(write);
        self.generations.truncate(write);
        self.revisions.truncate(write);
        Ok(())
    }
}

/// Active flight identity generations exposed to the typed hold update API.
/// Keys and tokens are parallel SoA columns, bounded by the configured active flight capacity.
pub(crate) struct FlightLifecycles {
    capacity: usize,
    keys: Vec<FlightKey>,
    generations: Vec<u64>,
    active_sorted: Vec<FlightKey>,
    merged_keys: Vec<FlightKey>,
    merged_generations: Vec<u64>,
    retired: Vec<FlightKey>,
}

fn compare_flight_keys(left: &FlightKey, right: &FlightKey) -> std::cmp::Ordering {
    (left.gufi, left.aircraft_address, left.centre, left.plan_id).cmp(&(
        right.gufi,
        right.aircraft_address,
        right.centre,
        right.plan_id,
    ))
}

impl FlightLifecycles {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            keys: Vec::with_capacity(capacity),
            generations: Vec::with_capacity(capacity),
            active_sorted: Vec::with_capacity(capacity),
            merged_keys: Vec::with_capacity(capacity),
            merged_generations: Vec::with_capacity(capacity),
            retired: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn generation(&self, key: FlightKey) -> Option<u64> {
        self.keys
            .binary_search_by(|candidate| compare_flight_keys(candidate, &key))
            .ok()
            .and_then(|index| self.generations.get(index).copied())
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    pub(crate) fn validate_generation(
        &self,
        key: FlightKey,
        generation: u64,
    ) -> Result<(), &'static str> {
        if self.generation(key) == Some(generation) {
            Ok(())
        } else {
            Err("hold update belongs to a stale or inactive flight lifecycle")
        }
    }

    pub(crate) fn reconcile(
        &mut self,
        active_keys: &[FlightKey],
        next_generation: &mut u64,
    ) -> Result<(), &'static str> {
        if self.keys.len() != self.generations.len() {
            return Err("flight lifecycle SoA invariant failed");
        }
        if active_keys.len() > self.capacity
            || self.keys.len() > self.capacity
            || self.active_sorted.capacity() < self.capacity
            || self.merged_keys.capacity() < self.capacity
            || self.merged_generations.capacity() < self.capacity
            || self.retired.capacity() < self.capacity
        {
            return Err("flight lifecycle input or scratch capacity exceeded");
        }
        self.active_sorted.clear();
        self.active_sorted.extend_from_slice(active_keys);
        for pair in self.active_sorted.windows(2) {
            if let [prior, current] = pair {
                match compare_flight_keys(prior, current) {
                    std::cmp::Ordering::Equal => return Err("duplicate active full flight key"),
                    std::cmp::Ordering::Greater => {
                        return Err("active lifecycle keys must be sorted by full identity")
                    }
                    std::cmp::Ordering::Less => {}
                }
            }
        }
        if self.keys.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| compare_flight_keys(left, right).is_gt())
        }) {
            return Err("flight lifecycle keys lost sorted invariant");
        }

        let mut old_index = 0usize;
        let mut active_index = 0usize;
        let mut additions = 0usize;
        let mut merge_comparisons = 0usize;
        while old_index < self.keys.len() && active_index < self.active_sorted.len() {
            merge_comparisons = merge_comparisons
                .checked_add(1)
                .ok_or("flight lifecycle merge work counter overflow")?;
            let old_key = self
                .keys
                .get(old_index)
                .ok_or("flight lifecycle key index invariant failed")?;
            let active_key = self
                .active_sorted
                .get(active_index)
                .ok_or("active lifecycle key index invariant failed")?;
            match compare_flight_keys(old_key, active_key) {
                std::cmp::Ordering::Less => old_index += 1,
                std::cmp::Ordering::Equal => {
                    old_index += 1;
                    active_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    additions = additions
                        .checked_add(1)
                        .ok_or("flight generation count overflow")?;
                    active_index += 1;
                }
            }
        }
        additions = additions
            .checked_add(self.active_sorted.len().saturating_sub(active_index))
            .ok_or("flight generation count overflow")?;
        let merge_work_limit = self
            .keys
            .len()
            .checked_add(self.active_sorted.len())
            .and_then(|work| work.checked_mul(2))
            .ok_or("flight lifecycle merge work bound overflow")?;
        if merge_comparisons > merge_work_limit {
            return Err("flight lifecycle preflight merge exceeded its C7 work bound");
        }
        let final_generation = next_generation
            .checked_add(u64::try_from(additions).map_err(|_| "flight generation count overflow")?)
            .ok_or("flight lifecycle generation exhausted")?;

        self.retired.clear();
        self.merged_keys.clear();
        self.merged_generations.clear();
        old_index = 0;
        active_index = 0;
        let mut merge_work = 0usize;
        while old_index < self.keys.len() || active_index < self.active_sorted.len() {
            merge_work = merge_work
                .checked_add(1)
                .ok_or("flight lifecycle merge work counter overflow")?;
            if merge_work > merge_work_limit {
                return Err("flight lifecycle merge exceeded its preflight C7 work bound");
            }
            let old_key = self.keys.get(old_index).copied();
            let active_key = self.active_sorted.get(active_index).copied();
            match (old_key, active_key) {
                (Some(old), Some(active)) => match compare_flight_keys(&old, &active) {
                    std::cmp::Ordering::Less => {
                        self.retired.push(old);
                        old_index += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        self.merged_keys.push(old);
                        self.merged_generations.push(
                            *self
                                .generations
                                .get(old_index)
                                .ok_or("flight lifecycle generation index invariant failed")?,
                        );
                        old_index += 1;
                        active_index += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        *next_generation = next_generation
                            .checked_add(1)
                            .ok_or("flight lifecycle generation exhausted")?;
                        self.merged_keys.push(active);
                        self.merged_generations.push(*next_generation);
                        active_index += 1;
                    }
                },
                (Some(old), None) => {
                    self.retired.push(old);
                    old_index += 1;
                }
                (None, Some(active)) => {
                    *next_generation = next_generation
                        .checked_add(1)
                        .ok_or("flight lifecycle generation exhausted")?;
                    self.merged_keys.push(active);
                    self.merged_generations.push(*next_generation);
                    active_index += 1;
                }
                (None, None) => break,
            }
        }
        if *next_generation != final_generation
            || self.merged_keys.len() != self.active_sorted.len()
            || self.merged_generations.len() != self.merged_keys.len()
        {
            return Err("flight lifecycle merge result invariant failed");
        }
        core::mem::swap(&mut self.keys, &mut self.merged_keys);
        core::mem::swap(&mut self.generations, &mut self.merged_generations);
        Ok(())
    }

    pub(crate) fn retired_keys(&self) -> &[FlightKey] {
        &self.retired
    }

    pub(crate) fn active_keys(&self) -> &[FlightKey] {
        &self.active_sorted
    }

    pub(crate) fn snapshot(&self) -> Result<(Vec<FlightKey>, Vec<u64>), &'static str> {
        if self.keys.len() != self.generations.len() {
            return Err("flight lifecycle SoA invariant failed");
        }
        Ok((self.keys.clone(), self.generations.clone()))
    }
}

impl HoldStore {
    pub(crate) fn new(capacity: usize) -> Self {
        let profile_capacity = capacity.saturating_mul(crate::route::HOLD_PROFILE_POINTS);
        let mut profile_points = Vec::with_capacity(profile_capacity);
        profile_points.resize(profile_capacity, crate::route::EMPTY_HOLD_ROUTE_POINT);
        Self {
            capacity,
            keys: Vec::with_capacity(capacity),
            fixes: Vec::with_capacity(capacity),
            directions: Vec::with_capacity(capacity),
            courses: Vec::with_capacity(capacity),
            turns: Vec::with_capacity(capacity),
            legs: Vec::with_capacity(capacity),
            predictions: Vec::with_capacity(capacity),
            phase_entry_ns: Vec::with_capacity(capacity),
            phase_last_ns: Vec::with_capacity(capacity),
            phase_speed_mmps: Vec::with_capacity(capacity),
            phase_models: Vec::with_capacity(capacity),
            efc_minutes: Vec::with_capacity(capacity),
            updated_receipt_ns: Vec::with_capacity(capacity),
            revisions: Vec::with_capacity(capacity),
            lifecycle_generations: Vec::with_capacity(capacity),
            profile_points,
            profile_states: Vec::with_capacity(capacity),
            profile_configs: Vec::with_capacity(capacity),
            profile_revisions: Vec::with_capacity(capacity),
            profile_scan_sequences: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    pub(crate) fn position(&self, key: FlightKey) -> Option<usize> {
        self.keys.iter().position(|&candidate| candidate == key)
    }

    pub(crate) fn key(&self, index: usize) -> Option<FlightKey> {
        self.keys.get(index).copied()
    }

    pub(crate) fn fix(&self, index: usize) -> Option<RouteFixInput> {
        self.fixes.get(index).copied()
    }

    pub(crate) fn turn(&self, index: usize) -> Option<HoldTurn> {
        self.turns.get(index).copied().flatten()
    }

    pub(crate) fn profile_points_mut(
        &mut self,
        index: usize,
    ) -> Option<
        &mut [oa_legacy_traj::core::trajectory::RoutePoint; crate::route::HOLD_PROFILE_POINTS],
    > {
        let start = index.checked_mul(crate::route::HOLD_PROFILE_POINTS)?;
        let end = start.checked_add(crate::route::HOLD_PROFILE_POINTS)?;
        self.profile_points.get_mut(start..end)?.try_into().ok()
    }

    pub(crate) fn cache_profile(
        &mut self,
        index: usize,
        state: HoldPhaseState,
        config: HoldPredictionIntent,
        revision: u64,
        scan_sequence: u64,
    ) -> Result<(), &'static str> {
        if self.keys.get(index).copied() != Some(state.key)
            || self.prediction(index) != Some(config)
            || self.revision(index) != Some(revision)
            || self.phase_state(index) != Some(state)
        {
            return Err("hold profile identity changed before cache publication");
        }
        *self
            .profile_states
            .get_mut(index)
            .ok_or("hold profile state column invariant failed")? = Some(state);
        *self
            .profile_configs
            .get_mut(index)
            .ok_or("hold profile config column invariant failed")? = Some(config);
        *self
            .profile_revisions
            .get_mut(index)
            .ok_or("hold profile revision column invariant failed")? = revision;
        *self
            .profile_scan_sequences
            .get_mut(index)
            .ok_or("hold profile scan column invariant failed")? = scan_sequence;
        Ok(())
    }

    pub(crate) fn cached_profile(
        &self,
        index: usize,
        state: HoldPhaseState,
        config: HoldPredictionIntent,
        revision: u64,
        scan_sequence: u64,
    ) -> Option<&[oa_legacy_traj::core::trajectory::RoutePoint; crate::route::HOLD_PROFILE_POINTS]>
    {
        if self.keys.get(index).copied() != Some(state.key)
            || self.prediction(index) != Some(config)
            || self.revision(index) != Some(revision)
            || self.phase_state(index) != Some(state)
            || self.profile_states.get(index).copied().flatten() != Some(state)
            || self.profile_configs.get(index).copied().flatten() != Some(config)
            || self.profile_revisions.get(index).copied() != Some(revision)
            || self.profile_scan_sequences.get(index).copied() != Some(scan_sequence)
        {
            return None;
        }
        let start = index.checked_mul(crate::route::HOLD_PROFILE_POINTS)?;
        let end = start.checked_add(crate::route::HOLD_PROFILE_POINTS)?;
        self.profile_points.get(start..end)?.try_into().ok()
    }

    pub(crate) fn prediction(&self, index: usize) -> Option<HoldPredictionIntent> {
        self.predictions.get(index).copied().flatten()
    }

    pub(crate) fn revision(&self, index: usize) -> Option<u64> {
        self.revisions.get(index).copied()
    }

    pub(crate) fn phase_state(&self, index: usize) -> Option<HoldPhaseState> {
        self.phase_models.get(index).copied().flatten()
    }

    pub(crate) fn update_phase(
        &mut self,
        index: usize,
        state: HoldPhaseState,
    ) -> Result<(), &'static str> {
        if self.keys.get(index).copied() != Some(state.key) {
            return Err("hold phase key/index mismatch");
        }
        *self
            .phase_entry_ns
            .get_mut(index)
            .ok_or("hold phase entry invariant failed")? = Some(state.entry_state_ns);
        *self
            .phase_last_ns
            .get_mut(index)
            .ok_or("hold phase time invariant failed")? = Some(state.last_advance_ns);
        *self
            .phase_speed_mmps
            .get_mut(index)
            .ok_or("hold phase speed invariant failed")? = Some(state.groundspeed_mmps);
        *self
            .phase_models
            .get_mut(index)
            .ok_or("hold phase model invariant failed")? = Some(state);
        Ok(())
    }

    pub(crate) fn push(
        &mut self,
        instruction: HoldInstruction,
        updated_receipt_ns: i64,
        revision: u64,
        lifecycle_generation: u64,
        phase_state: Option<HoldPhaseState>,
    ) -> Result<(), &'static str> {
        let len = self.len();
        if len >= self.capacity {
            return Err("hold view capacity exhausted");
        }
        if self.fixes.len() != len
            || self.directions.len() != len
            || self.courses.len() != len
            || self.turns.len() != len
            || self.legs.len() != len
            || self.predictions.len() != len
            || self.phase_entry_ns.len() != len
            || self.phase_last_ns.len() != len
            || self.phase_speed_mmps.len() != len
            || self.phase_models.len() != len
            || self.efc_minutes.len() != len
            || self.updated_receipt_ns.len() != len
            || self.revisions.len() != len
            || self.lifecycle_generations.len() != len
            || self.profile_states.len() != len
            || self.profile_configs.len() != len
            || self.profile_revisions.len() != len
            || self.profile_scan_sequences.len() != len
        {
            return Err("hold view SoA column invariant failed");
        }
        self.keys.push(instruction.flight_key);
        self.fixes.push(instruction.fix);
        self.directions.push(instruction.direction_from_fix_deg);
        self.courses.push(instruction.inbound_course_deg);
        self.turns.push(instruction.turn);
        self.legs.push(instruction.leg);
        self.predictions.push(instruction.prediction);
        self.phase_entry_ns
            .push(phase_state.map(|state| state.entry_state_ns));
        self.phase_last_ns
            .push(phase_state.map(|state| state.last_advance_ns));
        self.phase_speed_mmps
            .push(phase_state.map(|state| state.groundspeed_mmps));
        self.phase_models.push(phase_state);
        self.efc_minutes.push(instruction.efc_utc_minute);
        self.updated_receipt_ns.push(updated_receipt_ns);
        self.revisions.push(revision);
        self.lifecycle_generations.push(lifecycle_generation);
        self.profile_states.push(None);
        self.profile_configs.push(None);
        self.profile_revisions.push(0);
        self.profile_scan_sequences.push(0);
        Ok(())
    }

    pub(crate) fn replace(
        &mut self,
        index: usize,
        instruction: HoldInstruction,
        updated_receipt_ns: i64,
        revision: u64,
        lifecycle_generation: u64,
        phase_state: Option<HoldPhaseState>,
    ) -> Result<(), &'static str> {
        let len = self.len();
        if index >= len
            || self.fixes.len() != len
            || self.directions.len() != len
            || self.courses.len() != len
            || self.turns.len() != len
            || self.legs.len() != len
            || self.predictions.len() != len
            || self.phase_entry_ns.len() != len
            || self.phase_last_ns.len() != len
            || self.phase_speed_mmps.len() != len
            || self.phase_models.len() != len
            || self.efc_minutes.len() != len
            || self.updated_receipt_ns.len() != len
            || self.revisions.len() != len
            || self.lifecycle_generations.len() != len
            || self.profile_states.len() != len
            || self.profile_configs.len() != len
            || self.profile_revisions.len() != len
            || self.profile_scan_sequences.len() != len
        {
            return Err("hold view SoA column invariant failed");
        }
        let Some(key) = self.keys.get_mut(index) else {
            return Err("hold view index invariant failed");
        };
        *key = instruction.flight_key;
        let Some(fix) = self.fixes.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *fix = instruction.fix;
        let Some(direction) = self.directions.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *direction = instruction.direction_from_fix_deg;
        let Some(course) = self.courses.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *course = instruction.inbound_course_deg;
        let Some(turn) = self.turns.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *turn = instruction.turn;
        let Some(leg) = self.legs.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *leg = instruction.leg;
        *self
            .predictions
            .get_mut(index)
            .ok_or("hold SoA invariant failed")? = instruction.prediction;
        *self
            .phase_entry_ns
            .get_mut(index)
            .ok_or("hold SoA invariant failed")? = phase_state.map(|state| state.entry_state_ns);
        *self
            .phase_last_ns
            .get_mut(index)
            .ok_or("hold SoA invariant failed")? = phase_state.map(|state| state.last_advance_ns);
        *self
            .phase_speed_mmps
            .get_mut(index)
            .ok_or("hold SoA invariant failed")? = phase_state.map(|state| state.groundspeed_mmps);
        *self
            .phase_models
            .get_mut(index)
            .ok_or("hold phase model invariant failed")? = phase_state;
        let Some(efc) = self.efc_minutes.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *efc = instruction.efc_utc_minute;
        let Some(receipt) = self.updated_receipt_ns.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *receipt = updated_receipt_ns;
        let Some(stored_revision) = self.revisions.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *stored_revision = revision;
        let Some(stored_generation) = self.lifecycle_generations.get_mut(index) else {
            return Err("hold view SoA column invariant failed");
        };
        *stored_generation = lifecycle_generation;
        *self
            .profile_states
            .get_mut(index)
            .ok_or("hold profile state column invariant failed")? = None;
        *self
            .profile_configs
            .get_mut(index)
            .ok_or("hold profile config column invariant failed")? = None;
        *self
            .profile_revisions
            .get_mut(index)
            .ok_or("hold profile revision column invariant failed")? = 0;
        *self
            .profile_scan_sequences
            .get_mut(index)
            .ok_or("hold profile scan column invariant failed")? = 0;
        Ok(())
    }

    pub(crate) fn remove(&mut self, index: usize) -> Result<(), &'static str> {
        if index >= self.len()
            || self.fixes.len() != self.len()
            || self.directions.len() != self.len()
            || self.courses.len() != self.len()
            || self.turns.len() != self.len()
            || self.legs.len() != self.len()
            || self.predictions.len() != self.len()
            || self.phase_entry_ns.len() != self.len()
            || self.phase_last_ns.len() != self.len()
            || self.phase_speed_mmps.len() != self.len()
            || self.phase_models.len() != self.len()
            || self.efc_minutes.len() != self.len()
            || self.updated_receipt_ns.len() != self.len()
            || self.revisions.len() != self.len()
            || self.lifecycle_generations.len() != self.len()
            || self.profile_states.len() != self.len()
            || self.profile_configs.len() != self.len()
            || self.profile_revisions.len() != self.len()
            || self.profile_scan_sequences.len() != self.len()
            || self.profile_points.len()
                != self
                    .capacity
                    .saturating_mul(crate::route::HOLD_PROFILE_POINTS)
        {
            return Err("hold view SoA column invariant failed");
        }
        let profile_start = index
            .checked_mul(crate::route::HOLD_PROFILE_POINTS)
            .ok_or("hold profile index overflow")?;
        let profile_end = self
            .len()
            .checked_mul(crate::route::HOLD_PROFILE_POINTS)
            .ok_or("hold profile index overflow")?;
        let next_start = profile_start
            .checked_add(crate::route::HOLD_PROFILE_POINTS)
            .ok_or("hold profile index overflow")?;
        if next_start <= profile_end {
            self.profile_points
                .copy_within(next_start..profile_end, profile_start);
            let last_start = profile_end - crate::route::HOLD_PROFILE_POINTS;
            self.profile_points
                .get_mut(last_start..profile_end)
                .ok_or("hold profile tail invariant failed")?
                .fill(crate::route::EMPTY_HOLD_ROUTE_POINT);
        }
        self.keys.remove(index);
        self.fixes.remove(index);
        self.directions.remove(index);
        self.courses.remove(index);
        self.turns.remove(index);
        self.legs.remove(index);
        self.predictions.remove(index);
        self.phase_entry_ns.remove(index);
        self.phase_last_ns.remove(index);
        self.phase_speed_mmps.remove(index);
        self.phase_models.remove(index);
        self.efc_minutes.remove(index);
        self.updated_receipt_ns.remove(index);
        self.revisions.remove(index);
        self.lifecycle_generations.remove(index);
        self.profile_states.remove(index);
        self.profile_configs.remove(index);
        self.profile_revisions.remove(index);
        self.profile_scan_sequences.remove(index);
        Ok(())
    }

    pub(crate) fn remove_keys(&mut self, retired: &[FlightKey]) -> Result<(), &'static str> {
        let len = self.len();
        let expected_profile_points = self
            .capacity
            .checked_mul(crate::route::HOLD_PROFILE_POINTS)
            .ok_or("hold retirement profile capacity overflow")?;
        if len > self.capacity
            || retired.len() > self.capacity
            || self.profile_points.len() != expected_profile_points
        {
            return Err("hold retirement count exceeded its capacity");
        }
        if retired.is_empty() {
            return Ok(());
        }
        if self.fixes.len() != len
            || self.directions.len() != len
            || self.courses.len() != len
            || self.turns.len() != len
            || self.legs.len() != len
            || self.predictions.len() != len
            || self.phase_entry_ns.len() != len
            || self.phase_last_ns.len() != len
            || self.phase_speed_mmps.len() != len
            || self.phase_models.len() != len
            || self.efc_minutes.len() != len
            || self.updated_receipt_ns.len() != len
            || self.revisions.len() != len
            || self.lifecycle_generations.len() != len
            || self.profile_states.len() != len
            || self.profile_configs.len() != len
            || self.profile_revisions.len() != len
            || self.profile_scan_sequences.len() != len
        {
            return Err("hold retirement SoA column invariant failed");
        }
        let mut write = 0usize;
        for read in 0..len {
            let key = *self
                .keys
                .get(read)
                .ok_or("hold retirement key invariant failed")?;
            if retired
                .binary_search_by(|candidate| compare_flight_keys(candidate, &key))
                .is_ok()
            {
                continue;
            }
            if write != read {
                macro_rules! copy_column {
                    ($field:ident) => {{
                        let value = *self
                            .$field
                            .get(read)
                            .ok_or("hold retirement source column invariant failed")?;
                        *self
                            .$field
                            .get_mut(write)
                            .ok_or("hold retirement destination column invariant failed")? = value;
                    }};
                }
                copy_column!(keys);
                copy_column!(fixes);
                copy_column!(directions);
                copy_column!(courses);
                copy_column!(turns);
                copy_column!(legs);
                copy_column!(predictions);
                copy_column!(phase_entry_ns);
                copy_column!(phase_last_ns);
                copy_column!(phase_speed_mmps);
                copy_column!(phase_models);
                copy_column!(efc_minutes);
                copy_column!(updated_receipt_ns);
                copy_column!(revisions);
                copy_column!(lifecycle_generations);
                copy_column!(profile_states);
                copy_column!(profile_configs);
                copy_column!(profile_revisions);
                copy_column!(profile_scan_sequences);
                let points = crate::route::HOLD_PROFILE_POINTS;
                let source_start = read
                    .checked_mul(points)
                    .ok_or("hold retirement profile source overflow")?;
                let source_end = source_start
                    .checked_add(points)
                    .ok_or("hold retirement profile source overflow")?;
                let destination_start = write
                    .checked_mul(points)
                    .ok_or("hold retirement profile destination overflow")?;
                self.profile_points
                    .copy_within(source_start..source_end, destination_start);
            }
            write += 1;
        }
        self.keys.truncate(write);
        self.fixes.truncate(write);
        self.directions.truncate(write);
        self.courses.truncate(write);
        self.turns.truncate(write);
        self.legs.truncate(write);
        self.predictions.truncate(write);
        self.phase_entry_ns.truncate(write);
        self.phase_last_ns.truncate(write);
        self.phase_speed_mmps.truncate(write);
        self.phase_models.truncate(write);
        self.efc_minutes.truncate(write);
        self.updated_receipt_ns.truncate(write);
        self.revisions.truncate(write);
        self.lifecycle_generations.truncate(write);
        self.profile_states.truncate(write);
        self.profile_configs.truncate(write);
        self.profile_revisions.truncate(write);
        self.profile_scan_sequences.truncate(write);
        let profile_start = write
            .checked_mul(crate::route::HOLD_PROFILE_POINTS)
            .ok_or("hold retirement profile tail overflow")?;
        self.profile_points
            .get_mut(profile_start..)
            .ok_or("hold retirement profile tail invariant failed")?
            .fill(crate::route::EMPTY_HOLD_ROUTE_POINT);
        Ok(())
    }

    pub(crate) fn view_rows(&self) -> Result<Vec<HoldViewRow>, &'static str> {
        let len = self.len();
        if self.fixes.len() != len
            || self.directions.len() != len
            || self.courses.len() != len
            || self.turns.len() != len
            || self.legs.len() != len
            || self.predictions.len() != len
            || self.phase_entry_ns.len() != len
            || self.phase_last_ns.len() != len
            || self.phase_speed_mmps.len() != len
            || self.phase_models.len() != len
            || self.efc_minutes.len() != len
            || self.updated_receipt_ns.len() != len
            || self.revisions.len() != len
            || self.lifecycle_generations.len() != len
            || self.profile_states.len() != len
            || self.profile_configs.len() != len
            || self.profile_revisions.len() != len
            || self.profile_scan_sequences.len() != len
        {
            return Err("hold view SoA column invariant failed");
        }
        let mut rows = Vec::with_capacity(len);
        for index in 0..len {
            let Some(flight_key) = self.keys.get(index).copied() else {
                return Err("hold view key column invariant failed");
            };
            let Some(fix) = self.fixes.get(index).copied() else {
                return Err("hold view fix column invariant failed");
            };
            let Some(direction_from_fix_deg) = self.directions.get(index).copied() else {
                return Err("hold view direction column invariant failed");
            };
            let Some(inbound_course_deg) = self.courses.get(index).copied() else {
                return Err("hold view course column invariant failed");
            };
            let Some(turn) = self.turns.get(index).copied() else {
                return Err("hold view turn column invariant failed");
            };
            let Some(leg) = self.legs.get(index).copied() else {
                return Err("hold view leg column invariant failed");
            };
            let Some(prediction) = self.predictions.get(index).copied() else {
                return Err("hold view prediction column invariant failed");
            };
            let Some(efc_utc_minute) = self.efc_minutes.get(index).copied() else {
                return Err("hold view EFC column invariant failed");
            };
            let Some(updated_receipt_ns) = self.updated_receipt_ns.get(index).copied() else {
                return Err("hold view receipt column invariant failed");
            };
            let Some(revision) = self.revisions.get(index).copied() else {
                return Err("hold view revision column invariant failed");
            };
            let Some(lifecycle_generation) = self.lifecycle_generations.get(index).copied() else {
                return Err("hold view generation column invariant failed");
            };
            let phase_state = self.phase_state(index);
            rows.push(HoldViewRow {
                store_index: index,
                instruction: HoldInstruction {
                    flight_key,
                    fix,
                    direction_from_fix_deg,
                    inbound_course_deg,
                    turn,
                    leg,
                    efc_utc_minute,
                    prediction,
                },
                updated_receipt_ns,
                revision,
                lifecycle_generation,
                phase_state,
                phase_snapshot: phase_state.and_then(|state| {
                    oa_legacy_traj::core::trajectory::hold_phase_predict(
                        &state,
                        state.last_advance_ns,
                    )
                    .ok()
                }),
            });
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod revision_tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn key(plan: [u8; 12]) -> FlightKey {
        FlightKey {
            gufi: [42; 16],
            aircraft_address: 0xabc101,
            centre: *b"KORD",
            plan_id: plan,
        }
    }

    #[test]
    fn revisions_are_independent_per_full_key_and_reclaimed_after_retirement() {
        let first = key(*b"PLAN00000001");
        let second = key(*b"PLAN00000002");
        let mut revisions = HoldRevisions::new(2);
        assert!(revisions.validate(first, 3, 9).is_ok());
        revisions.record(first, 3, 9).expect("record first");
        assert!(revisions.validate(second, 3, 1).is_ok());
        revisions.record(second, 3, 1).expect("record second");
        assert!(revisions.validate(first, 3, 8).is_err());
        assert!(revisions.validate(first, 3, 10).is_ok());
        revisions
            .remove_keys(&[first])
            .expect("reclaim retired revision");
        assert!(revisions.validate(key(*b"PLAN00000003"), 4, 1).is_ok());
    }

    // REQ: OA-ENC-023
    #[test]
    fn batch_retirement_compacts_hold_profiles_and_revision_rows_by_full_key() {
        let retired = key(*b"PLAN00000001");
        let active = key(*b"PLAN00000002");
        let retired_last = key(*b"PLAN00000003");
        let instruction = |flight_key| HoldInstruction {
            flight_key,
            fix: RouteFixInput {
                name: *b"FIXA\0\0\0\0",
                region: *b"K1\0\0",
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
            },
            direction_from_fix_deg: 90,
            inbound_course_deg: 270,
            turn: None,
            leg: None,
            efc_utc_minute: None,
            prediction: None,
        };
        let mut store = HoldStore::new(3);
        store
            .push(instruction(retired), 1, 7, 4, None)
            .expect("retired insert");
        store
            .push(instruction(active), 2, 1, 5, None)
            .expect("active insert");
        store
            .push(instruction(retired_last), 3, 8, 6, None)
            .expect("second retired insert");
        let mut active_marker = crate::route::EMPTY_HOLD_ROUTE_POINT;
        active_marker.lat_e7 = 412_345_678;
        store.profile_points_mut(1).expect("active profile slot")[0] = active_marker;
        store
            .remove_keys(&[retired, retired_last])
            .expect("batch retired cleanup");
        assert!(store.position(retired).is_none());
        assert!(store.position(retired_last).is_none());
        assert!(store.position(active).is_some());
        let rows = store.view_rows().expect("view");
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows.first(), Some(row) if row.store_index == 0));
        assert_eq!(
            store.profile_points_mut(0).expect("compacted profile")[0],
            active_marker
        );
        assert_eq!(
            store.profile_points_mut(2).expect("cleared tail profile")[0],
            crate::route::EMPTY_HOLD_ROUTE_POINT
        );

        let mut revisions = HoldRevisions::new(3);
        revisions
            .record(retired, 4, 7)
            .expect("retain high-water mark");
        revisions
            .record(active, 5, 1)
            .expect("retain active high-water mark");
        revisions
            .record(retired_last, 6, 8)
            .expect("retain second high-water mark");
        revisions
            .remove_keys(&[retired, retired_last])
            .expect("batch revision cleanup");
        assert!(revisions.validate(retired, 4, 6).is_ok());
        assert!(revisions.validate(retired_last, 6, 7).is_ok());
        assert!(revisions.validate(active, 5, 0).is_err());
    }

    #[test]
    fn lifecycle_generation_reclaims_retired_identity_capacity_and_rejects_replay() {
        let mut lifecycle = FlightLifecycles::new(1);
        let mut revisions = HoldRevisions::new(1);
        let mut next = 0;
        let first = key(*b"PLAN00000001");
        let second = key(*b"PLAN00000002");
        lifecycle
            .reconcile(&[first], &mut next)
            .expect("first active");
        let old_generation = lifecycle.generation(first).expect("first generation");
        revisions
            .record(first, old_generation, 7)
            .expect("first revision");
        lifecycle
            .reconcile(&[second], &mut next)
            .expect("retire first, add second");
        assert_eq!(lifecycle.retired_keys(), &[first]);
        revisions
            .remove_keys(&[first])
            .expect("reclaim retired revision");
        assert_eq!(lifecycle.generation(second), Some(old_generation + 1));
        assert_eq!(lifecycle.generation(first), None);
        lifecycle
            .reconcile(&[first], &mut next)
            .expect("reuse identity");
        let new_generation = lifecycle.generation(first).expect("new lifecycle");
        assert_ne!(Some(new_generation), Some(old_generation));
        assert!(revisions.validate(first, new_generation, 1).is_ok());
        assert!(lifecycle
            .validate_generation(first, old_generation)
            .is_err());

        let before_keys = lifecycle.snapshot().expect("snapshot").0;
        let before_next = next;
        assert!(lifecycle.reconcile(&[first, second], &mut next).is_err());
        assert_eq!(lifecycle.snapshot().expect("snapshot").0, before_keys);
        assert_eq!(next, before_next);

        let mut exhausted = FlightLifecycles::new(1);
        let mut last = u64::MAX;
        assert!(exhausted.reconcile(&[first], &mut last).is_err());
        assert!(exhausted.snapshot().expect("empty snapshot").0.is_empty());
        assert_eq!(last, u64::MAX);
    }

    // REQ: OA-ENC-023
    #[test]
    fn pair_sua_and_trial_borrow_the_same_cached_profile_bytes() {
        let flight_key = key(*b"PLAN00000001");
        let config = HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Direct,
            entry_state_ns: 1_000_000_000,
            course_basis: HoldEntryCourseBasis::WindOrHeadingUnknown,
            entry_capture_radius_mm: 100_000,
            inbound_true_course_cdeg: 9_000,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10_000_000_000,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 5_000,
        };
        let instruction = HoldInstruction {
            flight_key,
            fix: RouteFixInput {
                name: *b"HOLDFIX\0",
                region: *b"K1\0\0",
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
            },
            direction_from_fix_deg: 90,
            inbound_course_deg: 90,
            turn: Some(HoldTurn::Right),
            leg: None,
            efc_utc_minute: None,
            prediction: Some(config),
        };
        let phase = oa_legacy_traj::core::trajectory::hold_phase_start(
            oa_legacy_traj::core::trajectory::HoldPhaseInput {
                key: flight_key,
                entry_state_ns: config.entry_state_ns,
                fix_lat_e7: instruction.fix.latitude_e7,
                fix_lon_e7: instruction.fix.longitude_e7,
                inbound_true_course_cdeg: config.inbound_true_course_cdeg,
                turn: 2,
                groundspeed_mmps: 100_000,
                leg_duration_ms: config.leg_duration_ms,
                turn_rate_cdeg_s: config.turn_rate_cdeg_s,
                max_scan_gap_ns: config.max_scan_gap_ns,
                max_speed_error_mmps: config.max_speed_error_mmps,
            },
        )
        .expect("valid synthetic held phase")
        .0;
        let mut store = HoldStore::new(1);
        store
            .push(instruction, 1_000, 4, 7, Some(phase))
            .expect("insert one active hold");
        let points = store
            .profile_points_mut(0)
            .expect("reserved fixed profile buffer");
        for (index, point) in points.iter_mut().enumerate() {
            *point = crate::route::EMPTY_HOLD_ROUTE_POINT;
            point.lat_e7 = 410_000_000 + i32::try_from(index).expect("small test index");
        }
        store
            .cache_profile(0, phase, config, 4, 12)
            .expect("publish profile for one scan");
        let pair_view = store
            .cached_profile(0, phase, config, 4, 12)
            .expect("pair consumer view");
        let sua_view = store
            .cached_profile(0, phase, config, 4, 12)
            .expect("SUA consumer view");
        let trial_counterpart_view = store
            .cached_profile(0, phase, config, 4, 12)
            .expect("trial counterpart consumer view");
        assert!(core::ptr::eq(pair_view, sua_view));
        assert!(core::ptr::eq(pair_view, trial_counterpart_view));
        assert_eq!(pair_view.as_slice(), sua_view.as_slice());
        assert_eq!(pair_view.as_slice(), trial_counterpart_view.as_slice());
    }

    // REQ: OA-ENC-023
    #[test]
    fn inferred_entry_profile_is_one_shared_pair_sua_trial_buffer() {
        let flight_key = key(*b"PLAN00000001");
        let config = HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Parallel,
            entry_state_ns: 0,
            course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
            entry_capture_radius_mm: 100_000,
            inbound_true_course_cdeg: 0,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10_000_000_000,
            max_position_error_mm: 100_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 5_000,
        };
        let instruction = HoldInstruction {
            flight_key,
            fix: RouteFixInput {
                name: *b"HOLDFIX\0",
                region: *b"K1\0\0",
                latitude_e7: 410_000_000,
                longitude_e7: -870_000_000,
            },
            direction_from_fix_deg: 0,
            inbound_course_deg: 0,
            turn: Some(HoldTurn::Right),
            leg: None,
            efc_utc_minute: None,
            prediction: Some(config),
        };
        let phase = oa_legacy_traj::core::trajectory::hold_phase_start_entry(
            oa_legacy_traj::core::trajectory::HoldEntryPhaseInput {
                key: flight_key,
                crossing_state_ns: 1_000_000_000,
                available_at_ns: 1_500_000_000,
                start_lat_e7: instruction.fix.latitude_e7,
                start_lon_e7: instruction.fix.longitude_e7,
                start_heading_true_cdeg: 12_000,
                fix_lat_e7: instruction.fix.latitude_e7,
                fix_lon_e7: instruction.fix.longitude_e7,
                inbound_true_course_cdeg: config.inbound_true_course_cdeg,
                turn: 2,
                procedure: 2,
                groundspeed_mmps: 100_000,
                leg_duration_ms: config.leg_duration_ms,
                turn_rate_cdeg_s: config.turn_rate_cdeg_s,
                max_scan_gap_ns: config.max_scan_gap_ns,
                max_speed_error_mmps: config.max_speed_error_mmps,
            },
        )
        .expect("bounded parallel hold model")
        .0;
        let mut store = HoldStore::new(1);
        store
            .push(instruction, 1_500, 4, 7, Some(phase))
            .expect("insert inferred hold");
        let points = store.profile_points_mut(0).expect("startup route buffer");
        points.fill(crate::route::EMPTY_HOLD_ROUTE_POINT);
        store
            .cache_profile(0, phase, config, 4, 22)
            .expect("publish one inferred route for the scan");
        let pair = store
            .cached_profile(0, phase, config, 4, 22)
            .expect("pair uses cached profile");
        let sua = store
            .cached_profile(0, phase, config, 4, 22)
            .expect("SUA uses cached profile");
        let trial = store
            .cached_profile(0, phase, config, 4, 22)
            .expect("trial counterpart uses cached profile");
        assert!(core::ptr::eq(pair, sua));
        assert!(core::ptr::eq(pair, trial));
        assert_eq!(pair.as_slice(), sua.as_slice());
        assert_eq!(pair.as_slice(), trial.as_slice());
    }
}

#[cfg(test)]
mod entry_inference_tests {
    #![allow(clippy::expect_used)]

    use super::*;

    const FIX_LAT_E7: i32 = 410_000_000;
    const FIX_LON_E7: i32 = -870_000_000;

    fn key() -> FlightKey {
        FlightKey {
            gufi: [7; 16],
            aircraft_address: 0xabc101,
            centre: *b"KORD",
            plan_id: *b"HOLDPLAN0001",
        }
    }

    fn reports(course_deg: i32) -> (HoldEntryReport, HoldEntryReport) {
        // Synthetic no-wind straight segment, 200 m long and centered on the configured fix.
        let angle = f64::from(course_deg).to_radians();
        let (sin_angle, cos_angle) = oa_math::sin_cos(angle);
        let east = sin_angle * 100.0;
        let north = cos_angle * 100.0;
        let to_report = |sign: f64, sequence: u64, source_ns: i64| HoldEntryReport {
            key: key(),
            source_epoch: 3,
            sequence,
            source_ns,
            receipt_ns: source_ns + 500_000_000,
            lat_e7: FIX_LAT_E7
                - oa_ieee::round(north * sign / 111_194.926_644_558_74 * 10_000_000.0) as i32,
            lon_e7: FIX_LON_E7
                - oa_ieee::round(
                    east * sign / (111_194.926_644_558_74 * oa_math::cos(41.0_f64.to_radians()))
                        * 10_000_000.0,
                ) as i32,
            r95_mm: 10_000,
        };
        (
            to_report(1.0, 10, 10_000_000_000),
            to_report(-1.0, 11, 20_000_000_000),
        )
    }

    fn infer(course: i32, turn: HoldTurn) -> Result<HoldEntryEvidence, HoldEntryInferenceError> {
        let (previous, current) = reports(course);
        infer_hold_entry(
            previous,
            current,
            HoldEntryInferenceConfig {
                fix_lat_e7: FIX_LAT_E7,
                fix_lon_e7: FIX_LON_E7,
                inbound_true_course_cdeg: 0,
                turn,
                max_scan_gap_ns: 15_000_000_000,
                capture_radius_mm: 100_000,
                sector_ambiguity_cdeg: 500,
                course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
            },
        )
    }

    // REQ: OA-ENC-023
    #[test]
    fn inferred_entry_classifies_each_procedure_for_both_turn_directions() {
        assert_eq!(
            infer(270, HoldTurn::Right).expect("direct").procedure,
            HoldEntryProcedure::Direct
        );
        assert_eq!(
            infer(35, HoldTurn::Right).expect("teardrop").procedure,
            HoldEntryProcedure::Teardrop
        );
        assert_eq!(
            infer(120, HoldTurn::Right).expect("parallel").procedure,
            HoldEntryProcedure::Parallel
        );
        assert_eq!(
            infer(90, HoldTurn::Left).expect("direct").procedure,
            HoldEntryProcedure::Direct
        );
        assert_eq!(
            infer(325, HoldTurn::Left).expect("teardrop").procedure,
            HoldEntryProcedure::Teardrop
        );
        assert_eq!(
            infer(240, HoldTurn::Left).expect("parallel").procedure,
            HoldEntryProcedure::Parallel
        );
        for (course, turn) in [
            (0, HoldTurn::Right),
            (70, HoldTurn::Right),
            (180, HoldTurn::Right),
            (0, HoldTurn::Left),
            (180, HoldTurn::Left),
            (290, HoldTurn::Left),
            (74, HoldTurn::Right),
            (286, HoldTurn::Left),
        ] {
            assert_eq!(
                infer(course, turn),
                Err(HoldEntryInferenceError::AmbiguousSector)
            );
        }
    }

    // REQ: OA-ENC-023
    #[test]
    fn inferred_crossing_is_source_interpolated_but_only_available_at_second_receipt() {
        let evidence = infer(270, HoldTurn::Right).expect("synthetic crossing");
        assert_eq!(evidence.crossing_ns, 15_000_000_000);
        assert_eq!(evidence.available_at_ns, 20_500_000_000);
        assert!(evidence.available_at_ns > evidence.crossing_ns);
        assert_eq!(evidence.previous_sequence, 10);
        assert_eq!(evidence.current_sequence, 11);
        assert_eq!(evidence.combined_r95_mm, 20_000);
    }

    // REQ: OA-ENC-023
    #[test]
    fn inference_refuses_wind_unknown_epoch_sequence_gap_and_sector_boundary() {
        let (previous, current) = reports(270);
        let base = infer_hold_entry(
            previous,
            current,
            HoldEntryInferenceConfig {
                fix_lat_e7: FIX_LAT_E7,
                fix_lon_e7: FIX_LON_E7,
                inbound_true_course_cdeg: 0,
                turn: HoldTurn::Right,
                max_scan_gap_ns: 15_000_000_000,
                capture_radius_mm: 100_000,
                sector_ambiguity_cdeg: 500,
                course_basis: HoldEntryCourseBasis::WindOrHeadingUnknown,
            },
        );
        assert_eq!(base, Err(HoldEntryInferenceError::UnavailableCourseBasis));
        let mut wrong_epoch = current;
        wrong_epoch.source_epoch += 1;
        assert_eq!(
            infer_hold_entry(
                previous,
                wrong_epoch,
                HoldEntryInferenceConfig {
                    fix_lat_e7: FIX_LAT_E7,
                    fix_lon_e7: FIX_LON_E7,
                    inbound_true_course_cdeg: 0,
                    turn: HoldTurn::Right,
                    max_scan_gap_ns: 15_000_000_000,
                    capture_radius_mm: 100_000,
                    sector_ambiguity_cdeg: 500,
                    course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
                }
            ),
            Err(HoldEntryInferenceError::IdentityOrSequenceMismatch)
        );
        let mut wrong_sequence = current;
        wrong_sequence.sequence = previous.sequence;
        assert_eq!(
            infer_hold_entry(
                previous,
                wrong_sequence,
                HoldEntryInferenceConfig {
                    fix_lat_e7: FIX_LAT_E7,
                    fix_lon_e7: FIX_LON_E7,
                    inbound_true_course_cdeg: 0,
                    turn: HoldTurn::Right,
                    max_scan_gap_ns: 15_000_000_000,
                    capture_radius_mm: 100_000,
                    sector_ambiguity_cdeg: 500,
                    course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
                }
            ),
            Err(HoldEntryInferenceError::IdentityOrSequenceMismatch)
        );
        assert_eq!(
            infer(0, HoldTurn::Right),
            Err(HoldEntryInferenceError::AmbiguousSector)
        );
    }

    // REQ: OA-ENC-023
    #[test]
    fn inference_refuses_long_gap_and_capture_uncertainty_without_guessing() {
        let (previous, mut current) = reports(270);
        current.receipt_ns = previous.receipt_ns + 15_000_000_001;
        assert_eq!(
            infer_hold_entry(
                previous,
                current,
                HoldEntryInferenceConfig {
                    fix_lat_e7: FIX_LAT_E7,
                    fix_lon_e7: FIX_LON_E7,
                    inbound_true_course_cdeg: 0,
                    turn: HoldTurn::Right,
                    max_scan_gap_ns: 15_000_000_000,
                    capture_radius_mm: 100_000,
                    sector_ambiguity_cdeg: 500,
                    course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
                }
            ),
            Err(HoldEntryInferenceError::ScanGapExceeded)
        );
        let mut uncertain = reports(270).1;
        uncertain.r95_mm = 95_000;
        let (previous, _) = reports(270);
        assert_eq!(
            infer_hold_entry(
                previous,
                uncertain,
                HoldEntryInferenceConfig {
                    fix_lat_e7: FIX_LAT_E7,
                    fix_lon_e7: FIX_LON_E7,
                    inbound_true_course_cdeg: 0,
                    turn: HoldTurn::Right,
                    max_scan_gap_ns: 15_000_000_000,
                    capture_radius_mm: 100_000,
                    sector_ambiguity_cdeg: 500,
                    course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
                }
            ),
            Err(HoldEntryInferenceError::UncertaintyExceedsCapture)
        );
        let (mut offset_previous, mut offset_current) = reports(270);
        offset_previous.lat_e7 += 8_000;
        offset_current.lat_e7 += 8_000;
        assert_eq!(
            infer_hold_entry(
                offset_previous,
                offset_current,
                HoldEntryInferenceConfig {
                    fix_lat_e7: FIX_LAT_E7,
                    fix_lon_e7: FIX_LON_E7,
                    inbound_true_course_cdeg: 0,
                    turn: HoldTurn::Right,
                    max_scan_gap_ns: 15_000_000_000,
                    capture_radius_mm: 50_000,
                    sector_ambiguity_cdeg: 500,
                    course_basis: HoldEntryCourseBasis::SyntheticNoWindCourse,
                }
            ),
            Err(HoldEntryInferenceError::OutsideCaptureRadius)
        );
    }
}

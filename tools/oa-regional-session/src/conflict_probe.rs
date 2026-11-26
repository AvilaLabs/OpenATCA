//! Shell adapter for the first bounded conflict-probe slice.
//!
//! Both forecasts are produced here from the shared `oa-legacy-traj` predictor. A trial input is
//! borrowed and evaluated separately; this module never commits it to active flight state.

use oa_airspace::core::conflict_probe::{self, SuaTrajectoryKnots};
pub use oa_airspace::core::conflict_probe::{SuaProbeResult, SuaRefusal, SuaStatus};
use oa_airspace::core::geom::Area;
use oa_ca::probe::{probe_pair, ProbeResult, ProbeTrajectory, NO_PROBE_TIME_MS};
use oa_legacy_traj::core::trajectory as t;

pub const CONFLICT_PROBE_SCRATCH_BYTES: usize = 75_656;

/// Fixed startup-owned workspace. At 75,656 bytes it holds two bounded route-profile offset sets,
/// their shared pair timeline, two predictor output sets, and two aligned SoA trajectories.
#[repr(C)]
pub struct ConflictProbeScratch {
    offsets_a: [i64; t::MAX_PROFILE_SAMPLES],
    offsets_b: [i64; t::MAX_PROFILE_SAMPLES],
    pair_offsets: [i64; t::MAX_PAIR_PROFILE_SAMPLES],
    samples_a: [t::HorizonOutput; t::MAX_PAIR_PROFILE_SAMPLES],
    samples_b: [t::HorizonOutput; t::MAX_PAIR_PROFILE_SAMPLES],
    trajectory_a: ProbeTrajectory,
    trajectory_b: ProbeTrajectory,
}
const _: () = assert!(core::mem::size_of::<ConflictProbeScratch>() == CONFLICT_PROBE_SCRATCH_BYTES);

impl ConflictProbeScratch {
    /// Allocate no memory; construct this once in caller-owned startup storage and reuse it.
    pub const fn new() -> Self {
        const EMPTY_HORIZON: t::HorizonOutput = t::HorizonOutput {
            horizon_ms: 0,
            predicted_time_ms: 0,
            lat_e7: 0,
            lon_e7: 0,
            altitude_ft: t::NO_ALTITUDE_FT,
            groundspeed_mmps: 0,
            flags: 0,
            available: t::Availability::Unavailable,
            refusal: t::Refusal::None,
            reserved: [0; 2],
        };
        const EMPTY_TRAJECTORY: ProbeTrajectory = ProbeTrajectory {
            gufi: [0; 16],
            aircraft_address: 0,
            centre: [0; 4],
            plan_id: [0; 12],
            reserved_key_align: [0; 4],
            state_ms: 0,
            sample_count: 0,
            reserved_sample_count: [0; 6],
            time_ms: [0; oa_ca::probe::PROBE_MAX_SAMPLES],
            lat_e7: [0; oa_ca::probe::PROBE_MAX_SAMPLES],
            lon_e7: [0; oa_ca::probe::PROBE_MAX_SAMPLES],
            altitude_ft: [t::NO_ALTITUDE_FT; oa_ca::probe::PROBE_MAX_SAMPLES],
        };
        Self {
            offsets_a: [0; t::MAX_PROFILE_SAMPLES],
            offsets_b: [0; t::MAX_PROFILE_SAMPLES],
            pair_offsets: [0; t::MAX_PAIR_PROFILE_SAMPLES],
            samples_a: [EMPTY_HORIZON; t::MAX_PAIR_PROFILE_SAMPLES],
            samples_b: [EMPTY_HORIZON; t::MAX_PAIR_PROFILE_SAMPLES],
            trajectory_a: EMPTY_TRAJECTORY,
            trajectory_b: EMPTY_TRAJECTORY,
        }
    }
}

impl Default for ConflictProbeScratch {
    fn default() -> Self {
        Self::new()
    }
}

fn seed_trajectory(input: &t::TrajectoryIssueInput<'_>, output: &mut ProbeTrajectory) {
    output.gufi = input.key.gufi;
    output.aircraft_address = input.key.aircraft_address;
    output.centre = input.key.centre;
    output.plan_id = input.key.plan_id;
    output.reserved_key_align = [0; 4];
    output.state_ms = input.track.state_ms;
    output.sample_count = 0;
    output.reserved_sample_count = [0; 6];
}

fn fill_trajectory(
    input: &t::TrajectoryIssueInput<'_>,
    offsets: &[i64],
    samples: &[t::HorizonOutput],
    output: &mut ProbeTrajectory,
) -> bool {
    seed_trajectory(input, output);
    if offsets.len() != samples.len()
        || offsets.len() < 2
        || offsets.len() > oa_ca::probe::PROBE_MAX_SAMPLES
    {
        return false;
    }
    for (index, (offset, sample)) in offsets.iter().zip(samples).enumerate() {
        if sample.available != t::Availability::Available
            || sample.horizon_ms != *offset
            || input.track.state_ms.checked_add(*offset) != Some(sample.predicted_time_ms)
            || sample.reserved != [0; 2]
        {
            return false;
        }
        let (Some(time), Some(lat), Some(lon), Some(alt)) = (
            output.time_ms.get_mut(index),
            output.lat_e7.get_mut(index),
            output.lon_e7.get_mut(index),
            output.altitude_ft.get_mut(index),
        ) else {
            return false;
        };
        *time = sample.predicted_time_ms;
        *lat = sample.lat_e7;
        *lon = sample.lon_e7;
        *alt = sample.altitude_ft;
    }
    output.sample_count = match u16::try_from(offsets.len()) {
        Ok(value) => value,
        Err(_) => return false,
    };
    true
}

fn merge_offsets(a: &[i64], b: &[i64], out: &mut [i64]) -> Option<usize> {
    let (mut ai, mut bi, mut count) = (0usize, 0usize, 0usize);
    let mut prior = None;
    while ai < a.len() || bi < b.len() {
        let av = a.get(ai).copied();
        let bv = b.get(bi).copied();
        let next = match (av, bv) {
            (Some(x), Some(y)) if x < y => {
                ai += 1;
                x
            }
            (Some(x), Some(y)) if y < x => {
                bi += 1;
                y
            }
            (Some(x), Some(_)) => {
                ai += 1;
                bi += 1;
                x
            }
            (Some(x), None) => {
                ai += 1;
                x
            }
            (None, Some(y)) => {
                bi += 1;
                y
            }
            (None, None) => return None,
        };
        if prior != Some(next) {
            *out.get_mut(count)? = next;
            count += 1;
            prior = Some(next);
        }
    }
    Some(count)
}

fn build_pair(
    a: &t::TrajectoryIssueInput<'_>,
    b: &t::TrajectoryIssueInput<'_>,
    scratch: &mut ConflictProbeScratch,
) -> ProbeResult {
    seed_trajectory(a, &mut scratch.trajectory_a);
    seed_trajectory(b, &mut scratch.trajectory_b);
    // This first slice requires one synchronized state epoch. Do not merge equal relative
    // offsets from differently timed tracks: that would compare positions from different times.
    if a.track.state_ms != b.track.state_ms {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    }
    let (a_profile, a_count) = t::trajectory_profile_offsets(a, &mut scratch.offsets_a);
    let (b_profile, b_count) = t::trajectory_profile_offsets(b, &mut scratch.offsets_b);
    if a_profile.eta.refusal != t::Refusal::None || b_profile.eta.refusal != t::Refusal::None {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    }
    let (Some(a_offsets), Some(b_offsets)) = (
        scratch.offsets_a.get(..a_count),
        scratch.offsets_b.get(..b_count),
    ) else {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    };
    let Some(count) = merge_offsets(a_offsets, b_offsets, &mut scratch.pair_offsets) else {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    };
    let Some(offsets) = scratch.pair_offsets.get(..count) else {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    };
    let (Some(samples_a), Some(samples_b)) = (
        scratch.samples_a.get_mut(..count),
        scratch.samples_b.get_mut(..count),
    ) else {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    };
    let output_a = t::issue_trajectory_samples(a, offsets, samples_a);
    let output_b = t::issue_trajectory_samples(b, offsets, samples_b);
    if output_a.eta.refusal != t::Refusal::None
        || output_b.eta.refusal != t::Refusal::None
        || !fill_trajectory(a, offsets, samples_a, &mut scratch.trajectory_a)
        || !fill_trajectory(b, offsets, samples_b, &mut scratch.trajectory_b)
    {
        return probe_pair(&scratch.trajectory_a, &scratch.trajectory_b);
    }
    probe_pair(&scratch.trajectory_a, &scratch.trajectory_b)
}

/// Run the shared predictor for both active plans on their common 30-second/route-vertex time
/// axis, then compare the aligned samples. The caller supplies one startup-owned scratch value.
pub fn probe_active_pair(
    a: &t::TrajectoryIssueInput<'_>,
    b: &t::TrajectoryIssueInput<'_>,
    scratch: &mut ConflictProbeScratch,
) -> ProbeResult {
    build_pair(a, b, scratch)
}

/// Evaluate a detached amendment for `trial_side` (0 or 1) against the other active plan.
/// `active_a` and `active_b` are never mutated or replaced by the candidate.
pub fn probe_trial_plan(
    active_a: &t::TrajectoryIssueInput<'_>,
    active_b: &t::TrajectoryIssueInput<'_>,
    trial_side: u8,
    trial: &t::TrajectoryIssueInput<'_>,
    scratch: &mut ConflictProbeScratch,
) -> Option<ProbeResult> {
    match trial_side {
        0 => {
            if trial.track != active_a.track
                || trial.key.gufi != active_a.key.gufi
                || trial.key.aircraft_address != active_a.key.aircraft_address
                || trial.key.centre != active_a.key.centre
                || trial.cutoff_ms != active_a.cutoff_ms
                || trial.owner != active_a.owner
                || trial.clearance != active_a.clearance
                || trial.eta_target != active_a.eta_target
                || trial.max_track_age_ms != active_a.max_track_age_ms
            {
                return None;
            }
            Some(build_pair(trial, active_b, scratch))
        }
        1 => {
            if trial.track != active_b.track
                || trial.key.gufi != active_b.key.gufi
                || trial.key.aircraft_address != active_b.key.aircraft_address
                || trial.key.centre != active_b.key.centre
                || trial.cutoff_ms != active_b.cutoff_ms
                || trial.owner != active_b.owner
                || trial.clearance != active_b.clearance
                || trial.eta_target != active_b.eta_target
                || trial.max_track_age_ms != active_b.max_track_age_ms
            {
                return None;
            }
            Some(build_pair(active_a, trial, scratch))
        }
        _ => None,
    }
}

/// Preserve a stable sentinel for display adapters that need an unavailable row without a probe.
pub const NO_CONFLICT_PROBE_TIME_MS: i64 = NO_PROBE_TIME_MS;

/// Generate and evaluate one active route profile using caller-owned startup storage.
pub fn sample_active_trajectory<'a>(
    input: &t::TrajectoryIssueInput<'_>,
    scratch: &'a mut ConflictProbeScratch,
) -> Option<&'a ProbeTrajectory> {
    seed_trajectory(input, &mut scratch.trajectory_a);
    let (profile, count) = t::trajectory_profile_offsets(input, &mut scratch.offsets_a);
    if profile.eta.refusal != t::Refusal::None {
        return None;
    }
    let offsets = scratch.offsets_a.get(..count)?;
    let samples = scratch.samples_a.get_mut(..count)?;
    let evaluated = t::issue_trajectory_samples(input, offsets, samples);
    if evaluated.eta.refusal != t::Refusal::None
        || !fill_trajectory(input, offsets, samples, &mut scratch.trajectory_a)
    {
        return None;
    }
    Some(&scratch.trajectory_a)
}

/// Adapt the shared predictor value into the airspace core's plain SoA knot view.
pub fn probe_sua(
    trajectory: &ProbeTrajectory,
    area: &Area,
    schedule_known: bool,
    active_start_ms: i64,
    active_end_ms: i64,
    floor_msl_ft: i32,
    ceiling_msl_ft: i32,
) -> SuaProbeResult {
    let knots = SuaTrajectoryKnots {
        state_ms: trajectory.state_ms,
        sample_count: trajectory.sample_count,
        reserved_sample_count: trajectory.reserved_sample_count,
        time_ms: trajectory.time_ms,
        lat_e7: trajectory.lat_e7,
        lon_e7: trajectory.lon_e7,
        altitude_msl_ft: trajectory.altitude_ft,
    };
    conflict_probe::probe_sua(
        &knots,
        area,
        schedule_known,
        active_start_ms,
        active_end_ms,
        floor_msl_ft,
        ceiling_msl_ft,
    )
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::too_many_arguments
    )]

    use super::*;
    use oa_legacy_traj::core::trajectory::*;

    fn fix() -> FixIdentity {
        FixIdentity {
            ident: RouteIdentifier {
                len: 3,
                bytes: *b"FIX\0\0\0\0\0\0\0\0\0",
            },
            region: *b"US\0\0",
            scope: RouteIdentifier {
                len: 3,
                bytes: *b"ZAU\0\0\0\0\0\0\0\0\0",
            },
            class: 1,
        }
    }

    fn issue<'a>(
        addr: u32,
        gufi_byte: u8,
        plan: [u8; 12],
        lon_e7: i32,
        _route_end_e7: i32,
        altitude_ft: i32,
        vertical_rate_fpm: i32,
        points: &'a [RoutePoint],
    ) -> TrajectoryIssueInput<'a> {
        let key = FlightKey {
            gufi: [gufi_byte; 16],
            aircraft_address: addr,
            centre: *b"KZNY",
            plan_id: plan,
        };
        let fix = fix();
        TrajectoryIssueInput {
            key,
            route: RouteProvenance {
                source_ms: 999_000,
                available_ms: 999_500,
                message_id: 1,
                payload_sha256: [7; 32],
                key,
                reserved: [0; 4],
            },
            cutoff_ms: 1_000_000,
            max_track_age_ms: 5_000,
            owner: OwnerProvenance {
                source_ms: 999_000,
                available_ms: 999_500,
                message_id: 4,
                owner_unit: *b"ZAU1\0\0\0\0",
                owner_state: OwnerState::Known,
                reserved: [0; 7],
            },
            route_cursor: Some(ActiveRouteCursor {
                source_ms: 999_000,
                available_ms: 999_500,
                message_id: 2,
                route_payload_sha256: [7; 32],
                key,
                next_fix: fix,
                reserved: [0; 5],
            }),
            track: CurrentTrack {
                state_ms: 1_000_000,
                newest_source_ms: 999_000,
                available_ms: 1_000_000,
                message_id: 3,
                aircraft_address: addr,
                lat_e7: 0,
                lon_e7,
                altitude_ft,
                vertical_rate_fpm,
                groundspeed_mmps: 100_000,
            },
            clearance: Clearance {
                source_ms: 999_000,
                available_ms: 999_500,
                message_id: 5,
                min_altitude_ft: 0,
                max_altitude_ft: 40_000,
                altitude_at_fix_ft: NO_ALTITUDE_FT,
                form: ClearanceForm::Window,
                reserved: [0; 3],
            },
            eta_target: EtaTarget {
                present: false,
                fix,
            },
            points,
        }
    }

    fn point(lon_e7: i32) -> RoutePoint {
        RoutePoint {
            lat_e7: 0,
            lon_e7,
            min_altitude_ft: NO_ALTITUDE_FT,
            max_altitude_ft: NO_ALTITUDE_FT,
            min_speed_mmps: NO_SPEED_MMPS,
            max_speed_mmps: NO_SPEED_MMPS,
            fix: fix(),
            altitude_constraint_kind: AltitudeConstraintKind::None,
            speed_constraint_kind: SpeedConstraintKind::None,
            geometry: PathGeometry::GreatCircleLeg,
            speed_reference: SpeedReference::GroundSpeed,
            reserved: [0; 1],
        }
    }

    // REQ: OA-ENC-023
    #[test]
    fn shared_predictor_adapter_detects_converging_and_diverging_pairs() {
        let pa = [point(200_000_000)];
        let pb = [point(-200_000_000)];
        let a = issue(1, 1, *b"PLAN-A\0\0\0\0\0\0", 0, 200_000_000, 10_000, 0, &pa);
        let b = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            2_000_000,
            -200_000_000,
            10_000,
            0,
            &pb,
        );
        let mut scratch = ConflictProbeScratch::new();
        let converging = probe_active_pair(&a, &b, &mut scratch);
        assert_eq!(converging.status, oa_ca::probe::ProbeStatus::Loss);
        assert_ne!(converging.cpa_ms, NO_PROBE_TIME_MS);

        let pb_out = [point(202_000_000)];
        let b_out = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            2_000_000,
            202_000_000,
            10_000,
            0,
            &pb_out,
        );
        let diverging = probe_active_pair(&a, &b_out, &mut scratch);
        assert_eq!(diverging.status, oa_ca::probe::ProbeStatus::Clear);
        assert_ne!(diverging.cpa_ms, NO_PROBE_TIME_MS);
    }

    // REQ: OA-ENC-023
    #[test]
    fn detached_trial_can_resolve_or_create_without_mutating_active_inputs() {
        let pa = [point(200_000_000)];
        let pb = [point(-200_000_000)];
        let a = issue(1, 1, *b"PLAN-A\0\0\0\0\0\0", 0, 200_000_000, 10_000, 0, &pa);
        let b = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            2_000_000,
            -200_000_000,
            10_000,
            0,
            &pb,
        );
        let trial_a_points = [point(-200_000_000)];
        let trial_a = issue(
            1,
            1,
            *b"TRIAL-A\0\0\0\0\0",
            0,
            -200_000_000,
            10_000,
            0,
            &trial_a_points,
        );
        let mut scratch = ConflictProbeScratch::new();
        let active_a_before = a.track;
        let resolved =
            probe_trial_plan(&a, &b, 0, &trial_a, &mut scratch).expect("valid detached trial");
        assert_eq!(resolved.status, oa_ca::probe::ProbeStatus::Clear);
        assert_eq!(a.track, active_a_before);

        let p_div_a = [point(200_000_000)];
        let p_div_b = [point(202_000_000)];
        let div_a = issue(
            1,
            1,
            *b"PLAN-A\0\0\0\0\0\0",
            0,
            200_000_000,
            10_000,
            0,
            &p_div_a,
        );
        let div_b = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            2_000_000,
            202_000_000,
            10_000,
            0,
            &p_div_b,
        );
        let trial_b_points = [point(-200_000_000)];
        let trial_b = issue(
            2,
            2,
            *b"TRIAL-B\0\0\0\0\0",
            2_000_000,
            -200_000_000,
            10_000,
            0,
            &trial_b_points,
        );
        let created = probe_trial_plan(&div_a, &div_b, 1, &trial_b, &mut scratch)
            .expect("valid detached trial");
        assert_eq!(created.status, oa_ca::probe::ProbeStatus::Loss);
    }

    // REQ: OA-ENC-023
    #[test]
    fn pair_timeline_contains_both_flights_route_vertices() {
        let a_corner = point(1_000_000);
        let mut a_end = point(10_000_000);
        a_end.lat_e7 = 10_000_000;
        a_end.fix.ident.bytes = *b"END\0\0\0\0\0\0\0\0\0";
        let b_corner = point(2_000_000);
        let mut b_end = point(10_000_000);
        b_end.lat_e7 = 10_000_000;
        b_end.fix.ident.bytes = *b"END\0\0\0\0\0\0\0\0\0";
        let a_points = [a_corner, a_end];
        let b_points = [b_corner, b_end];
        let a = issue(1, 1, *b"PLAN-A\0\0\0\0\0\0", 0, 0, 10_000, 0, &a_points);
        let b = issue(2, 2, *b"PLAN-B\0\0\0\0\0\0", 0, 0, 10_000, 0, &b_points);
        let mut scratch = ConflictProbeScratch::new();

        let result = probe_active_pair(&a, &b, &mut scratch);

        assert_ne!(result.status, oa_ca::probe::ProbeStatus::Unavailable);
        let count = usize::from(scratch.trajectory_a.sample_count);
        let times = scratch.trajectory_a.time_ms.get(..count).unwrap();
        assert!(times
            .iter()
            .any(|time| (time - 1_000_000 - 111_195).abs() <= 5));
        assert!(times
            .iter()
            .any(|time| (time - 1_000_000 - 222_390).abs() <= 5));
        assert_eq!(
            scratch.trajectory_a.time_ms.get(..count),
            scratch.trajectory_b.time_ms.get(..count)
        );
    }

    // REQ: OA-ENC-023
    #[test]
    fn pair_refuses_different_track_epochs_without_relative_offset_alignment() {
        let points_a = [point(200_000_000)];
        let points_b = [point(200_000_000)];
        let a = issue(
            1,
            1,
            *b"PLAN-A\0\0\0\0\0\0",
            0,
            200_000_000,
            10_000,
            0,
            &points_a,
        );
        let mut b = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            0,
            200_000_000,
            10_000,
            0,
            &points_b,
        );
        b.track.state_ms += 1_000;
        b.track.available_ms += 1_000;
        let mut scratch = ConflictProbeScratch::new();

        let result = probe_active_pair(&a, &b, &mut scratch);

        assert_eq!(result.status, oa_ca::probe::ProbeStatus::Unavailable);
        assert_eq!(result.refusal, oa_ca::probe::ProbeRefusal::InvalidTimeAxis);
    }

    // REQ: OA-ENC-023
    #[test]
    fn sua_requires_known_schedule_detects_entry_and_refuses_subgrid_crossing() {
        let crossing_ring = [
            (-100_000, 3_900_000),
            (-100_000, 4_200_000),
            (100_000, 4_200_000),
            (100_000, 3_900_000),
        ];
        let crossing_rings: [&[(i32, i32)]; 1] = [&crossing_ring];
        let crossing_area = oa_airspace::core::geom::build_area_e7(&crossing_rings, 0.0)
            .expect("small synthetic crossing polygon");
        let points = [point(200_000_000)];
        let input = issue(
            1,
            1,
            *b"PLAN-A\0\0\0\0\0\0",
            0,
            200_000_000,
            10_000,
            0,
            &points,
        );
        let mut scratch = ConflictProbeScratch::new();
        let trajectory =
            sample_active_trajectory(&input, &mut scratch).expect("valid shared route profile");
        let inside = probe_sua(
            trajectory,
            &crossing_area,
            true,
            1_000_000,
            2_200_000,
            9_000,
            11_000,
        );
        assert_eq!(inside.status, SuaStatus::Entry);
        assert!(inside.entry_ms > 1_300_000 && inside.entry_ms < 1_600_000);

        // The polygon library does not define boundary containment. A profile whose sampled
        // point quantizes onto an exact edge must not be reported as an unambiguous Entry.
        let mut boundary = *trajectory;
        for latitude in boundary
            .lat_e7
            .iter_mut()
            .take(usize::from(boundary.sample_count))
        {
            *latitude = -100_000;
        }
        let boundary_result = probe_sua(
            &boundary,
            &crossing_area,
            true,
            1_000_000,
            2_200_000,
            9_000,
            11_000,
        );
        assert_eq!(boundary_result.status, SuaStatus::Unavailable);
        assert_eq!(boundary_result.refusal, SuaRefusal::UnsampledPossibleEntry);

        // A partial timeline cannot certify an active schedule after its last knot as Clear.
        let mut truncated = *trajectory;
        truncated.sample_count = 2;
        truncated.time_ms[1] = truncated.state_ms + 30_000;
        let after_tail = probe_sua(
            &truncated,
            &crossing_area,
            true,
            1_040_000,
            1_050_000,
            9_000,
            11_000,
        );
        assert_eq!(after_tail.status, SuaStatus::Unavailable);
        assert_eq!(after_tail.refusal, SuaRefusal::InvalidWindow);

        let mut wide_gap = *trajectory;
        wide_gap.sample_count = 2;
        wide_gap.time_ms[1] = wide_gap.state_ms + 1_200_000;
        let gap_result = probe_sua(
            &wide_gap,
            &crossing_area,
            true,
            1_000_000,
            1_010_000,
            9_000,
            11_000,
        );
        assert_eq!(gap_result.status, SuaStatus::Unavailable);
        assert_eq!(gap_result.refusal, SuaRefusal::InvalidWindow);

        let mut no_segments = truncated;
        no_segments.sample_count = 1;
        let no_segment_result = probe_sua(
            &no_segments,
            &crossing_area,
            true,
            1_000_000,
            1_010_000,
            9_000,
            11_000,
        );
        assert_eq!(no_segment_result.status, SuaStatus::Unavailable);
        assert_eq!(no_segment_result.refusal, SuaRefusal::InvalidWindow);

        // A sub-grid polygon is crossed between two profile samples. It is deliberately not
        // claimed Clear or assigned a fabricated entry time; finer crossing localization is
        // outside this first slice.
        let narrow_ring = [
            (-100_000, 3_999_950),
            (-100_000, 4_000_050),
            (100_000, 4_000_050),
            (100_000, 3_999_950),
        ];
        let narrow_rings: [&[(i32, i32)]; 1] = [&narrow_ring];
        let narrow_area = oa_airspace::core::geom::build_area_e7(&narrow_rings, 0.0)
            .expect("small synthetic sub-grid polygon");
        let narrow = probe_sua(
            trajectory,
            &narrow_area,
            true,
            1_000_000,
            2_200_000,
            9_000,
            11_000,
        );
        assert_eq!(narrow.status, SuaStatus::Unavailable);
        assert_eq!(narrow.refusal, SuaRefusal::UnsampledPossibleEntry);

        let distant_ring = [
            (-100_000, 100_000_000),
            (-100_000, 101_000_000),
            (100_000, 101_000_000),
            (100_000, 100_000_000),
        ];
        let distant_rings: [&[(i32, i32)]; 1] = [&distant_ring];
        let distant_area = oa_airspace::core::geom::build_area_e7(&distant_rings, 0.0)
            .expect("small synthetic distant polygon");
        let far_outside = probe_sua(
            trajectory,
            &distant_area,
            true,
            1_000_000,
            2_200_000,
            9_000,
            11_000,
        );
        assert_eq!(far_outside.status, SuaStatus::Clear);

        let area = crossing_area;
        let outside = probe_sua(trajectory, &area, true, 2_300_000, 2_500_000, 9_000, 11_000);
        assert_eq!(outside.status, SuaStatus::Clear);
        let unknown = probe_sua(
            trajectory, &area, false, 1_000_000, 1_500_000, 9_000, 11_000,
        );
        assert_eq!(unknown.status, SuaStatus::Unavailable);
        assert_eq!(unknown.refusal, SuaRefusal::UnknownSchedule);
    }

    // REQ: OA-ENC-023
    #[test]
    fn vertical_only_conflict_is_reported_by_shared_predictor_adapter() {
        let pa = [point(200_000_000)];
        let pb = [point(200_000_000)];
        let a = issue(1, 1, *b"PLAN-A\0\0\0\0\0\0", 0, 200_000_000, 10_000, 0, &pa);
        let b = issue(
            2,
            2,
            *b"PLAN-B\0\0\0\0\0\0",
            100_000,
            200_000_000,
            12_000,
            -600,
            &pb,
        );
        let mut scratch = ConflictProbeScratch::new();
        let out = probe_active_pair(&a, &b, &mut scratch);
        assert_eq!(out.status, oa_ca::probe::ProbeStatus::Loss);
    }
}

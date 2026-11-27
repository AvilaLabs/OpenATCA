#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_legacy_traj::core::trajectory::{
    FlightKey, PerformanceEnvelope, PerformanceSource, PerformanceStatus,
};
use oa_regional_session::cli::synthetic_model;
use oa_regional_session::flight::FieldAsOf;
use oa_regional_session::holding::{
    HoldEntryProcedure, HoldInstruction, HoldLegLength, HoldLegUnit, HoldPredictionIntent,
    HoldTurn, HoldUpdate,
};
use oa_regional_session::runtime::{
    regional_new, regional_select_automation_mode, regional_step, regional_step_with_performance,
    regional_trial_route_probe, AutomationDegradation, AutomationModeError,
    EffectiveAutomationMode, HoldCommandDisposition, RegionalAutomationMode,
    TrialRouteProbeRequest, MAX_PERFORMANCE_ENVELOPES,
};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{parse_event_line, ReceiptBatch};

const SECOND: i64 = 1_000_000_000;
const ROUTE: &str = "FIX:FIXA:K1:1:- DCT:FIXB:K1:1:-";
const ROUTE_HASH: &str = "2f6fb15ca1faf3d7d3f4f082096b670efc91dbe6f4d44c0ac4a57c713e181840";

struct HoldAuditHarness {
    root: std::path::PathBuf,
    registry_path: std::path::PathBuf,
    audit_path: std::path::PathBuf,
    registry_sha256: [u8; 32],
    checkpoint: oa_audit_core::Checkpoint,
    last_command_receipt_ns: i64,
}

impl HoldAuditHarness {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let suffix = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "oa-hold-runtime-{name}-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let registry_path = root.join("principals.txt");
        let audit_path = root.join("audit.log");
        let registry = b"OACERT-LOCAL-PRINCIPALS-V1\n71\toperate\n72\tmonitor\n";
        std::fs::write(&registry_path, registry).unwrap();
        Self {
            root,
            registry_path,
            audit_path,
            registry_sha256: oa_sha256::core::sha256::sha256(registry),
            checkpoint: oa_audit_core::genesis(),
            last_command_receipt_ns: 0,
        }
    }

    fn apply(
        &mut self,
        runtime: &mut oa_regional_session::runtime::RegionalRuntime,
        update: HoldUpdate,
        not_before_ns: i64,
    ) -> Result<oa_regional_session::runtime::HoldCommandResult, String> {
        let command_receipt_ns = not_before_ns.max(
            self.last_command_receipt_ns
                .checked_add(1)
                .ok_or("synthetic command receipt overflow")?,
        );
        let result = oa_regional_session::commands::dispatch_hold_command(
            runtime,
            update,
            oa_regional_session::commands::HoldCommandAuthority {
                principal_id: 71,
                registry_path: &self.registry_path,
                expected_registry_sha256: self.registry_sha256,
                audit_path: &self.audit_path,
                expected_checkpoint: self.checkpoint,
                command_receipt_ns,
            },
        );
        if let Ok(outcome) = &result {
            self.checkpoint = outcome.checkpoint;
            self.last_command_receipt_ns = command_receipt_ns;
        }
        result
    }
}

impl Drop for HoldAuditHarness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// REQ: OA-ENC-023
#[test]
fn runtime_pair_probe_and_flight_lists_use_the_same_admitted_scan() {
    let mut engine = runtime();
    let mut batch = start();
    batch.events.insert(
        10,
        row(
            SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tIDENTITY\tABC101\tKORD\tKLAX",
        ),
    );
    batch.events.insert(
        11,
        row(
            SECOND,
            10,
            "FLIGHT",
            2,
            0xabc102,
            "FLIGHT\tIDENTITY\tABC102\tKORD\tKJFK",
        ),
    );
    let tentative = regional_step(&mut engine, &batch).unwrap();
    assert!(tentative.surveillance.tracks.is_empty());
    let frame = regional_step(&mut engine, &confirmation_batch()).unwrap();
    assert_eq!(frame.aircraft_list.len(), 2);
    assert_eq!(frame.departure_list.len(), 2);
    assert_eq!(frame.pair_probes.len(), 1);
    assert_eq!(frame.pair_probe_input_refusals, 0);
    assert_eq!(frame.pair_probes_unavailable, 0);
    assert_eq!(frame.pair_probes_unexamined, 0);
    assert_eq!(frame.pair_probes[0].status, oa_ca::probe::ProbeStatus::Loss);
    let request = TrialRouteProbeRequest {
        candidate_route: row(
            2 * SECOND,
            5,
            "ROUTE",
            1,
            0xabc101,
            &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
        ),
        counterpart_gufi: 2,
    };
    let trial = regional_trial_route_probe(&mut engine, request).unwrap();
    assert_eq!(trial, frame.pair_probes[0]);
    assert_eq!(
        regional_trial_route_probe(&mut engine, request).unwrap(),
        trial
    );
}

// REQ: OA-ENC-023
#[test]
fn explicit_hold_view_assign_amend_cancel_and_suppress_unmodeled_trajectory() {
    let mut engine = runtime_with_continuous_sua();
    let _ = regional_step(&mut engine, &start()).unwrap();
    let frame = regional_step(&mut engine, &confirmation_batch()).unwrap();
    let active = *frame.aircraft_list.first().unwrap();
    let key = oa_regional_session::route::flight_key(&active);
    let lifecycle_generation = frame.hold_lifecycle_generation(key).unwrap();
    assert!(frame.hold_view.is_empty());
    assert_eq!(frame.pair_probes[0].status, oa_ca::probe::ProbeStatus::Loss);

    let instruction = HoldInstruction {
        flight_key: key,
        fix: oa_regional_session::RouteFixInput {
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
    let mut wrong_key = key;
    wrong_key.plan_id = *b"OTHERPLAN001";
    let mut audit = HoldAuditHarness::new("explicit-hold");
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction: HoldInstruction {
                    flight_key: wrong_key,
                    ..instruction
                }
            },
            2 * SECOND,
        )
        .is_err());

    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation: lifecycle_generation.saturating_add(1),
                revision: 1,
                instruction,
            },
            2 * SECOND,
        )
        .is_err());
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction: HoldInstruction {
                    fix: oa_regional_session::RouteFixInput {
                        latitude_e7: 1_000_000_000,
                        ..instruction.fix
                    },
                    ..instruction
                }
            },
            2 * SECOND,
        )
        .is_err());

    audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction,
            },
            2 * SECOND,
        )
        .unwrap();
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 2,
                instruction
            },
            2 * SECOND,
        )
        .is_err());
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Amend {
                lifecycle_generation,
                revision: 1,
                instruction
            },
            2 * SECOND,
        )
        .is_err());
    let trial_request = TrialRouteProbeRequest {
        candidate_route: row(
            2 * SECOND,
            5,
            "ROUTE",
            1,
            0xabc101,
            &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
        ),
        counterpart_gufi: 2,
    };
    assert!(regional_trial_route_probe(&mut engine, trial_request).is_err());
    let view = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869976000\t10000\t100000\t0\t0",
                ),
                row(
                    3 * SECOND,
                    6,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869924000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(view.hold_view.len(), 1);
    assert_eq!(view.hold_view[0].instruction.efc_utc_minute, None);
    assert_eq!(view.hold_view[0].updated_receipt_ns, 2 * SECOND);
    assert_eq!(view.hold_view[0].revision, 1);
    assert_eq!(view.hold_view[0].lifecycle_generation, lifecycle_generation);
    assert_eq!(view.pair_probes.len(), 1);
    assert_eq!(
        view.pair_probes[0].status,
        oa_ca::probe::ProbeStatus::Unavailable
    );
    assert_eq!(view.pair_probes_unavailable, 1);
    assert_eq!(view.pair_hold_probe_unavailable.len(), 1);
    assert_eq!(
        view.pair_hold_probe_unavailable[0].reason,
        oa_regional_session::holding::HoldProbeUnavailableReason::MissingAcceptedEntry
    );
    assert_eq!(view.sua_hold_probe_unavailable.len(), 1);
    assert_eq!(view.sua_hold_probe_unavailable[0].flight, key);
    assert_eq!(view.sua_hold_probe_unavailable[0].boundary_index, 0);
    assert!(view.sua_probes_unavailable >= 1);

    let amended = HoldInstruction {
        direction_from_fix_deg: 45,
        inbound_course_deg: 225,
        turn: Some(HoldTurn::Left),
        leg: Some(HoldLegLength {
            value: 4,
            unit: HoldLegUnit::Miles,
        }),
        efc_utc_minute: Some(0),
        prediction: Some(HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Parallel,
            entry_state_ns: 0,
            course_basis: oa_regional_session::holding::HoldEntryCourseBasis::SyntheticNoWindCourse,
            entry_capture_radius_mm: 100_000,
            inbound_true_course_cdeg: 22_500,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 5_000,
        }),
        ..instruction
    };
    audit
        .apply(
            &mut engine,
            HoldUpdate::Amend {
                lifecycle_generation,
                revision: 2,
                instruction: amended,
            },
            3 * SECOND,
        )
        .unwrap();
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Amend {
                lifecycle_generation,
                revision: 3,
                instruction: HoldInstruction {
                    efc_utc_minute: Some(1440),
                    ..amended
                }
            },
            3 * SECOND,
        )
        .is_err());
    let view = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    4 * SECOND,
                    7,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869964000\t10000\t100000\t0\t0",
                ),
                row(
                    4 * SECOND,
                    8,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869936000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(view.hold_view.len(), 1);
    assert_eq!(view.hold_view[0].instruction.efc_utc_minute, Some(0));
    assert_eq!(view.hold_view[0].instruction.turn, Some(HoldTurn::Left));
    assert_eq!(view.hold_view[0].instruction.leg, amended.leg);
    assert_eq!(view.hold_view[0].updated_receipt_ns, 3 * SECOND);
    assert_eq!(view.hold_view[0].revision, 2);
    assert_eq!(view.hold_view[0].lifecycle_generation, lifecycle_generation);
    assert_eq!(view.pair_hold_probe_unavailable.len(), 1);
    assert_eq!(
        view.pair_hold_probe_unavailable[0].reason,
        oa_regional_session::holding::HoldProbeUnavailableReason::UnsupportedEntryProcedure
    );

    audit
        .apply(
            &mut engine,
            HoldUpdate::Cancel {
                lifecycle_generation,
                revision: 3,
                flight_key: key,
            },
            4 * SECOND,
        )
        .unwrap();
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Cancel {
                lifecycle_generation,
                revision: 4,
                flight_key: key
            },
            4 * SECOND,
        )
        .is_err());
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Amend {
                lifecycle_generation,
                revision: 2,
                instruction: amended
            },
            4 * SECOND,
        )
        .is_err());
    assert!(audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction
            },
            4 * SECOND,
        )
        .is_err());
    let view = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 5 * SECOND,
            source_clock_regressions: 0,
            events: vec![],
        },
    )
    .unwrap();
    assert!(view.hold_view.is_empty());
    assert!(view.pair_hold_probe_unavailable.is_empty());
}

// REQ: OA-ENC-023
#[test]
fn duplicate_active_address_withholds_shared_track_from_pair_and_sua_probes() {
    let mut engine = runtime_with_continuous_sua();
    let mut initial = start();
    initial.events.insert(
        10,
        row(
            SECOND,
            20,
            "FLIGHT",
            3,
            0xabc101,
            "FLIGHT\tPLAN\tPLAN00000003\t2",
        ),
    );
    initial.events.insert(
        11,
        row(SECOND, 21, "FLIGHT", 3, 0xabc101, "FLIGHT\tADDRESS"),
    );
    regional_step(&mut engine, &initial).unwrap();
    let frame = regional_step(&mut engine, &confirmation_batch()).unwrap();
    assert_eq!(frame.aircraft_list.len(), 3);
    assert_eq!(frame.active_flight_keys.len(), 3);
    assert_eq!(frame.pair_probes.len(), 3);
    assert!(frame.pair_probes.iter().all(|probe| {
        probe.status == oa_ca::probe::ProbeStatus::Unavailable
            && probe.refusal == oa_ca::probe::ProbeRefusal::InvalidIdentity
    }));
    assert!(frame.sua_probes.iter().any(|probe| {
        probe.flight_gufi == 1
            && probe.result.status == oa_airspace::core::conflict_probe::SuaStatus::Unavailable
            && probe.result.refusal
                == oa_airspace::core::conflict_probe::SuaRefusal::AmbiguousIdentity
    }));
    assert!(frame.sua_probes.iter().any(|probe| {
        probe.flight_gufi == 3
            && probe.result.status == oa_airspace::core::conflict_probe::SuaStatus::Unavailable
            && probe.result.refusal
                == oa_airspace::core::conflict_probe::SuaRefusal::AmbiguousIdentity
    }));
}

// REQ: OA-ENC-023
#[test]
fn active_hold_lifecycle_survives_missing_tracks_and_retires_only_with_plan() {
    let mut engine = runtime_with_continuous_sua();
    let first = confirmed(&mut engine);
    let active = first
        .aircraft_list
        .iter()
        .find(|view| view.gufi == 1)
        .copied()
        .unwrap();
    let key = oa_regional_session::route::flight_key(&active);
    let lifecycle_generation = first.hold_lifecycle_generation(key).unwrap();
    let instruction = HoldInstruction {
        flight_key: key,
        fix: oa_regional_session::RouteFixInput {
            name: *b"FIXA\0\0\0\0",
            region: *b"K1\0\0",
            latitude_e7: 410_000_000,
            longitude_e7: -870_000_000,
        },
        direction_from_fix_deg: 90,
        inbound_course_deg: 270,
        turn: None,
        leg: None,
        efc_utc_minute: Some(300),
        prediction: None,
    };
    let mut audit = HoldAuditHarness::new("track-outage-retention");
    audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction,
            },
            2 * SECOND,
        )
        .unwrap();

    for receipt in [3 * SECOND, 4 * SECOND] {
        let outage = regional_step(
            &mut engine,
            &ReceiptBatch {
                receipt_ns: receipt,
                source_clock_regressions: 0,
                events: if receipt == 3 * SECOND {
                    vec![row(receipt, 3, "SURVEILLANCE", 0, 0, "FEED\tDOWN\t0")]
                } else {
                    Vec::new()
                },
            },
        )
        .unwrap();
        assert!(outage.surveillance.tracks.is_empty());
        assert_eq!(outage.hold_view.len(), 1);
        assert_eq!(outage.hold_view[0].instruction.flight_key, key);
        assert_eq!(outage.hold_view[0].revision, 1);
        assert_eq!(
            outage.hold_view[0].lifecycle_generation,
            lifecycle_generation
        );
        assert!(outage.aircraft_list.iter().any(|view| view.gufi == 1));
        assert!(outage.active_flight_keys.contains(&key));
        assert_eq!(outage.pair_probes.len(), 1);
        assert_eq!(
            outage.pair_probes[0].status,
            oa_ca::probe::ProbeStatus::Unavailable
        );
        assert_eq!(
            outage.pair_probes[0].refusal,
            oa_ca::probe::ProbeRefusal::MissingTrack
        );
        assert!(outage.sua_probes.iter().any(|probe| {
            probe.flight_gufi == 1
                && probe.result.status == oa_airspace::core::conflict_probe::SuaStatus::Unavailable
                && probe.result.refusal
                    == oa_airspace::core::conflict_probe::SuaRefusal::MissingTrack
        }));
    }

    let recovered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 5 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    5 * SECOND,
                    4,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869976000\t10000\t100000\t0\t0",
                ),
                row(
                    5 * SECOND,
                    5,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869924000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(recovered.hold_view.len(), 1);
    assert_eq!(
        recovered.hold_view[0].lifecycle_generation,
        lifecycle_generation
    );

    let rejected_completion = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 6 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                6 * SECOND,
                9,
                "FLIGHT",
                1,
                0xabc101,
                "FLIGHT\tPLAN\tPLAN00000001\t4",
            )],
        },
    )
    .unwrap();
    assert!(rejected_completion
        .flight_operations
        .iter()
        .any(|outcome| { matches!(outcome.operation, Some(oa_flight::Outcome::Refused(_))) }));
    assert!(rejected_completion.active_flight_keys.contains(&key));
    assert_eq!(rejected_completion.hold_view.len(), 1);

    let retired = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 7 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                7 * SECOND,
                10,
                "FLIGHT",
                1,
                0xabc101,
                "FLIGHT\tPLAN\tPLAN00000003\t2",
            )],
        },
    )
    .unwrap();
    assert!(!retired.active_flight_keys.contains(&key));
    assert!(retired.hold_view.is_empty());
}

// REQ: OA-AUDIT-010, OA-ENC-023
#[test]
fn accepted_direct_hold_is_reconciled_and_sampled_by_pair_and_sua_runtime() {
    let hold_key = FlightKey {
        gufi: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        aircraft_address: 0xabc101,
        centre: *b"KORD",
        plan_id: *b"PLAN00000001",
    };
    let (hold_state, _) = oa_legacy_traj::core::trajectory::hold_phase_start(
        oa_legacy_traj::core::trajectory::HoldPhaseInput {
            key: hold_key,
            entry_state_ns: 2 * SECOND,
            fix_lat_e7: 410_000_000,
            fix_lon_e7: -869_988_000,
            inbound_true_course_cdeg: 9_000,
            turn: 2,
            groundspeed_mmps: 100_000,
            leg_duration_ms: 30_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_speed_error_mmps: 5_000,
        },
    )
    .unwrap();
    let target =
        oa_legacy_traj::core::trajectory::hold_phase_predict(&hold_state, 92 * SECOND).unwrap();
    let mut engine = runtime_with_sua_bounds(
        target.lat_e7 - 50_000,
        target.lat_e7 + 50_000,
        target.lon_e7 - 60_000,
        target.lon_e7 + 60_000,
    );
    let mut initial = start();
    initial.events.insert(
        10,
        row(
            SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tIDENTITY\tABC101\tKORD\tKLAX",
        ),
    );
    initial.events.insert(
        11,
        row(
            SECOND,
            10,
            "FLIGHT",
            2,
            0xabc102,
            "FLIGHT\tIDENTITY\tABC102\tKORD\tKJFK",
        ),
    );
    let _ = regional_step(&mut engine, &initial).unwrap();
    let frame = regional_step(&mut engine, &confirmation_batch()).unwrap();
    assert_eq!(frame.aircraft_list.len(), 2);
    let active = *frame.aircraft_list.first().unwrap();
    let key = oa_regional_session::route::flight_key(&active);
    let lifecycle_generation = frame.hold_lifecycle_generation(key).unwrap();
    let track = frame
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == active.address)
        .copied()
        .unwrap();
    let heading = oa_math::rem_euclid(
        oa_math::atan2(f64::from(track.east_mmps), f64::from(track.north_mmps))
            * (180.0 / core::f64::consts::PI),
        360.0,
    );
    let course_cdeg = oa_ieee::round(heading * 100.0) as u16 % 36_000;
    let instruction = HoldInstruction {
        flight_key: key,
        fix: oa_regional_session::RouteFixInput {
            name: *b"HOLDFIX\0",
            region: *b"K1\0\0",
            latitude_e7: track.lat_e7,
            longitude_e7: track.lon_e7,
        },
        direction_from_fix_deg: course_cdeg / 100,
        inbound_course_deg: course_cdeg / 100,
        turn: Some(HoldTurn::Right),
        leg: None,
        efc_utc_minute: None,
        prediction: Some(HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Direct,
            entry_state_ns: track.state_ns,
            course_basis: oa_regional_session::holding::HoldEntryCourseBasis::WindOrHeadingUnknown,
            entry_capture_radius_mm: 100_000,
            inbound_true_course_cdeg: course_cdeg,
            leg_duration_ms: 30_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 5_000,
        }),
    };
    let audit_root =
        std::env::temp_dir().join(format!("oa-hold-command-runtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&audit_root);
    std::fs::create_dir_all(&audit_root).unwrap();
    let registry_path = audit_root.join("principals.txt");
    let audit_path = audit_root.join("audit.log");
    let registry_bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n71\toperate\n72\tmonitor\n";
    std::fs::write(&registry_path, registry_bytes).unwrap();
    let registry_pin = oa_sha256::core::sha256::sha256(registry_bytes);
    let update = HoldUpdate::Assign {
        lifecycle_generation,
        revision: 1,
        instruction,
    };
    let denied = oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        update,
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 72,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: oa_audit_core::genesis(),
            command_receipt_ns: 2 * SECOND,
        },
    );
    assert!(denied.is_err());
    assert!(!audit_path.exists());
    let mut mismatched_key = key;
    mismatched_key.plan_id = *b"OTHERPLAN001";
    assert!(oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        HoldUpdate::Assign {
            lifecycle_generation,
            revision: 1,
            instruction: HoldInstruction {
                flight_key: mismatched_key,
                ..instruction
            },
        },
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: oa_audit_core::genesis(),
            command_receipt_ns: 2 * SECOND,
        },
    )
    .is_err());
    assert!(!audit_path.exists());

    let failed_audit_path = audit_root.join("missing-parent").join("audit.log");
    let audit_refusal = oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        update,
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &failed_audit_path,
            expected_checkpoint: oa_audit_core::genesis(),
            command_receipt_ns: 2 * SECOND,
        },
    );
    assert!(audit_refusal.is_err());
    assert!(!failed_audit_path.exists());

    let committed = oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        update,
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: oa_audit_core::genesis(),
            command_receipt_ns: (5 * SECOND) / 2,
        },
    )
    .unwrap();
    assert_eq!(committed.disposition, HoldCommandDisposition::Applied);
    assert_eq!(committed.checkpoint.count, 1);
    let mut audited_hold_intent = false;
    oa_audit_log::shell::read_records(&audit_path, committed.checkpoint, &mut |record| {
        audited_hold_intent = record.event.action == oa_audit_core::Action::HoldCommand
            && record.event.principal == 71
            && record.event.time_ns == u64::try_from((5 * SECOND) / 2).unwrap()
            && String::from_utf8_lossy(
                &record.event.details[..usize::from(record.event.details_len)],
            )
            .contains("operation=ASSIGN;lifecycle=");
    })
    .unwrap();
    assert!(audited_hold_intent);
    let mut checkpoint = committed.checkpoint;
    assert!(oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        HoldUpdate::Amend {
            lifecycle_generation,
            revision: 1,
            instruction,
        },
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: checkpoint,
            command_receipt_ns: (5 * SECOND) / 2,
        },
    )
    .is_err());
    let mut refused_amendment = instruction;
    refused_amendment.direction_from_fix_deg =
        refused_amendment.direction_from_fix_deg.saturating_add(1);
    assert!(oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        HoldUpdate::Amend {
            lifecycle_generation,
            revision: 2,
            instruction: refused_amendment,
        },
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &failed_audit_path,
            expected_checkpoint: checkpoint,
            command_receipt_ns: 4 * SECOND,
        },
    )
    .is_err());

    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    active.address,
                    "POS\t410000000\t-869976000\t10000\t100000\t0\t0",
                ),
                row(
                    3 * SECOND,
                    6,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869924000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(next.hold_view.len(), 1);
    assert_eq!(next.hold_view[0].instruction, instruction);
    assert_eq!(next.hold_phase_profiles_built, 1);
    assert_eq!(
        next.hold_view[0].phase_snapshot.unwrap().at_ns,
        next.surveillance
            .tracks
            .iter()
            .find(|track| track.address == active.address)
            .unwrap()
            .state_ns
    );
    assert_eq!(
        next.hold_view[0].phase_state.unwrap().entry_state_ns,
        track.state_ns
    );
    assert!(next.pair_hold_probe_unavailable.is_empty());
    assert_eq!(
        next.pair_probes.len(),
        1,
        "aircraft={} tracks={} flights={:?}",
        next.aircraft_list.len(),
        next.surveillance.tracks.len(),
        next.flights
    );
    assert_eq!(
        next.pair_probes_unavailable,
        0,
        "pair results: {:?}, track epochs: {:?}",
        next.pair_probes,
        next.surveillance
            .tracks
            .iter()
            .map(|track| (track.address, track.state_ns))
            .collect::<Vec<_>>()
    );
    assert_eq!(next.sua_hold_probe_unavailable.len(), 0);
    let held_entry = next
        .sua_probes
        .iter()
        .find(|row| {
            row.flight_gufi == 1
                && row.result.status == oa_airspace::core::conflict_probe::SuaStatus::Entry
        })
        .expect("the held path must enter the bounded SUA from outside");
    assert!(
        ((hold_state.entry_state_ns + 80 * SECOND) / 1_000_000
            ..=(hold_state.entry_state_ns + 100 * SECOND) / 1_000_000)
            .contains(&held_entry.result.entry_ms),
        "held SUA entry time should follow the future phase, got {} ms",
        held_entry.result.entry_ms
    );

    let later = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    4 * SECOND,
                    7,
                    "SURVEILLANCE",
                    1,
                    active.address,
                    "POS\t410000000\t-869964000\t10000\t100000\t0\t0",
                ),
                row(
                    4 * SECOND,
                    8,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869936000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(later.hold_view.len(), 1);
    assert_eq!(later.hold_phase_profiles_built, 1);
    assert_eq!(
        later.hold_view[0].phase_snapshot.unwrap().at_ns,
        later
            .surveillance
            .tracks
            .iter()
            .find(|track| track.address == active.address)
            .unwrap()
            .state_ns
    );
    assert!(later.pair_hold_probe_unavailable.is_empty());
    assert_eq!(later.pair_probes.len(), 1);
    assert!(later.sua_hold_probe_unavailable.is_empty());
    let trial_request = TrialRouteProbeRequest {
        candidate_route: row(
            4 * SECOND,
            7,
            "ROUTE",
            2,
            0xabc102,
            "FILED_ROUTE\tPLAN00000002\tFIXB\tFIX:FIXA:K1:1:- DCT:FIXB:K1:1:-",
        ),
        counterpart_gufi: 1,
    };
    let trial = regional_trial_route_probe(&mut engine, trial_request).unwrap();
    assert_eq!(
        regional_trial_route_probe(&mut engine, trial_request).unwrap(),
        trial
    );
    let reentry_track = later
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == active.address)
        .copied()
        .unwrap();
    let reentry_heading = oa_math::rem_euclid(
        oa_math::atan2(
            f64::from(reentry_track.east_mmps),
            f64::from(reentry_track.north_mmps),
        ) * (180.0 / core::f64::consts::PI),
        360.0,
    );
    let reentry_course_cdeg = oa_ieee::round(reentry_heading * 100.0) as u16 % 36_000;
    let reentry_instruction = HoldInstruction {
        fix: oa_regional_session::RouteFixInput {
            latitude_e7: reentry_track.lat_e7,
            longitude_e7: reentry_track.lon_e7,
            ..instruction.fix
        },
        direction_from_fix_deg: reentry_course_cdeg / 100,
        inbound_course_deg: reentry_course_cdeg / 100,
        prediction: Some(HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Direct,
            entry_state_ns: reentry_track.state_ns,
            course_basis: oa_regional_session::holding::HoldEntryCourseBasis::WindOrHeadingUnknown,
            entry_capture_radius_mm: 100_000,
            inbound_true_course_cdeg: reentry_course_cdeg,
            leg_duration_ms: 30_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 5_000,
        }),
        ..instruction
    };
    let amended = oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        HoldUpdate::Amend {
            lifecycle_generation,
            revision: 2,
            instruction: reentry_instruction,
        },
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: checkpoint,
            command_receipt_ns: 5 * SECOND,
        },
    )
    .unwrap();
    assert_eq!(amended.disposition, HoldCommandDisposition::Applied);
    checkpoint = amended.checkpoint;
    let reentered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 5 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    5 * SECOND,
                    9,
                    "SURVEILLANCE",
                    1,
                    active.address,
                    "POS\t410000000\t-869952000\t10000\t100000\t0\t0",
                ),
                row(
                    5 * SECOND,
                    10,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869948000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(reentered.hold_view.len(), 1);
    assert_eq!(reentered.hold_phase_profiles_built, 1);
    assert_eq!(
        reentered.hold_view[0].phase_state.unwrap().entry_state_ns,
        reentry_track.state_ns
    );
    assert!(reentered.pair_hold_probe_unavailable.is_empty());
    let cancelled_command = oa_regional_session::commands::dispatch_hold_command(
        &mut engine,
        HoldUpdate::Cancel {
            lifecycle_generation,
            revision: 3,
            flight_key: key,
        },
        oa_regional_session::commands::HoldCommandAuthority {
            principal_id: 71,
            registry_path: &registry_path,
            expected_registry_sha256: registry_pin,
            audit_path: &audit_path,
            expected_checkpoint: checkpoint,
            command_receipt_ns: 6 * SECOND,
        },
    )
    .unwrap();
    assert_eq!(
        cancelled_command.disposition,
        HoldCommandDisposition::Applied
    );
    checkpoint = cancelled_command.checkpoint;
    assert_eq!(checkpoint.count, 3);
    let mut operation_count = 0usize;
    oa_audit_log::shell::read_records(&audit_path, checkpoint, &mut |record| {
        let details =
            String::from_utf8_lossy(&record.event.details[..usize::from(record.event.details_len)]);
        if record.event.action == oa_audit_core::Action::HoldCommand
            && (details.contains("operation=ASSIGN;")
                || details.contains("operation=AMEND;")
                || details.contains("operation=CANCEL;"))
        {
            operation_count = operation_count.saturating_add(1);
        }
    })
    .unwrap();
    assert_eq!(operation_count, 3);
    let cancelled = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 6 * SECOND,
            source_clock_regressions: 0,
            events: vec![],
        },
    )
    .unwrap();
    assert!(cancelled.hold_view.is_empty());
    assert_eq!(cancelled.hold_phase_profiles_built, 0);
    let _ = std::fs::remove_dir_all(audit_root);
}

// REQ: OA-ENC-023
#[test]
fn inferred_parallel_entry_uses_the_normal_shared_scan_profile() {
    let mut engine = runtime_with_continuous_sua();
    let _ = regional_step(&mut engine, &start()).unwrap();
    let frame = regional_step(&mut engine, &confirmation_batch()).unwrap();
    let active = frame
        .aircraft_list
        .iter()
        .find(|view| view.address == 0xabc101)
        .copied()
        .unwrap();
    let key = oa_regional_session::route::flight_key(&active);
    let lifecycle_generation = frame.hold_lifecycle_generation(key).unwrap();
    let fix = oa_regional_session::RouteFixInput {
        name: *b"HOLDFIX\0",
        region: *b"K1\0\0",
        // Midpoint of the source-pinned synthetic report segment below.
        latitude_e7: 409_997_750,
        longitude_e7: -869_982_850,
    };
    let instruction = HoldInstruction {
        flight_key: key,
        fix,
        direction_from_fix_deg: 0,
        inbound_course_deg: 0,
        turn: Some(HoldTurn::Right),
        leg: Some(HoldLegLength {
            value: 1,
            unit: HoldLegUnit::Minutes,
        }),
        efc_utc_minute: None,
        prediction: Some(HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Parallel,
            entry_state_ns: 0,
            course_basis: oa_regional_session::holding::HoldEntryCourseBasis::SyntheticNoWindCourse,
            entry_capture_radius_mm: 500_000,
            inbound_true_course_cdeg: 0,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 10_000,
        }),
    };
    let mut hold_audit = HoldAuditHarness::new("parallel-hold");
    hold_audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction,
            },
            2 * SECOND,
        )
        .unwrap();

    let current = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                3 * SECOND,
                5,
                "SURVEILLANCE",
                1,
                0xabc101,
                "POS\t409995500\t-869977700\t10000\t86603\t-50000\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(current.hold_phase_profiles_built, 1);
    assert_eq!(current.hold_view.len(), 1);
    assert_eq!(current.hold_view[0].phase_state.unwrap().entry_model, 1);
    assert_eq!(current.pair_hold_probe_unavailable.len(), 0);
    assert_eq!(current.sua_hold_probe_unavailable.len(), 0);
    assert!(current.pair_probes_unavailable <= current.pair_probes.len() as u64);
}

// REQ: OA-ENC-023
#[test]
fn inferred_teardrop_entry_uses_receipt_gated_shared_scan_profile() {
    use oa_regional_session::input::EventKind;

    fn set_track(
        batch: &mut ReceiptBatch,
        address: u32,
        latitude_e7: i32,
        longitude_e7: i32,
        east_mmps: i32,
        north_mmps: i32,
    ) {
        let event = batch
            .events
            .iter_mut()
            .find(|event| {
                event.identity.aircraft_address == address
                    && matches!(event.kind, EventKind::Position { .. })
            })
            .expect("fixture has the requested synthetic position");
        let EventKind::Position {
            latitude_e7: event_lat,
            longitude_e7: event_lon,
            east_mmps: event_east,
            north_mmps: event_north,
            ..
        } = &mut event.kind
        else {
            unreachable!("filtered position event")
        };
        *event_lat = latitude_e7;
        *event_lon = longitude_e7;
        *event_east = east_mmps;
        *event_north = north_mmps;
    }

    const FIX_LAT_E7: i32 = 410_000_000;
    const FIX_LON_E7: i32 = -870_000_000;
    // Synthetic no-wind course 35°: 50 m is approximately 3,684 E7 north and
    // 3,419 E7 east at this latitude. The accepted reports straddle the fix at 100 m/s.
    const NORTH_50M_E7: i32 = 3_684;
    const EAST_50M_E7: i32 = 3_419;
    const EAST_35_MMPS: i32 = 57_358;
    const NORTH_35_MMPS: i32 = 81_915;

    let hold_key = oa_legacy_traj::core::trajectory::FlightKey {
        gufi: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        aircraft_address: 0xabc101,
        centre: *b"KORD",
        plan_id: *b"PLAN00000001",
    };
    let (predicted_hold, _) = oa_legacy_traj::core::trajectory::hold_phase_start_entry(
        oa_legacy_traj::core::trajectory::HoldEntryPhaseInput {
            key: hold_key,
            crossing_state_ns: 2 * SECOND + SECOND / 2,
            available_at_ns: 3 * SECOND,
            start_lat_e7: FIX_LAT_E7,
            start_lon_e7: FIX_LON_E7,
            start_heading_true_cdeg: 3_500,
            fix_lat_e7: FIX_LAT_E7,
            fix_lon_e7: FIX_LON_E7,
            inbound_true_course_cdeg: 0,
            turn: 2,
            procedure: 3,
            groundspeed_mmps: 100_000,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_speed_error_mmps: 10_000,
        },
    )
    .unwrap();
    let future_target =
        oa_legacy_traj::core::trajectory::hold_phase_predict(&predicted_hold, 92 * SECOND).unwrap();
    let mut engine = runtime_with_sua_bounds(
        future_target.lat_e7 - 50_000,
        future_target.lat_e7 + 50_000,
        future_target.lon_e7 - 60_000,
        future_target.lon_e7 + 60_000,
    );
    let mut initial = start();
    set_track(
        &mut initial,
        0xabc101,
        FIX_LAT_E7 - 3 * NORTH_50M_E7,
        FIX_LON_E7 - 3 * EAST_50M_E7,
        EAST_35_MMPS,
        NORTH_35_MMPS,
    );
    let first = regional_step(&mut engine, &initial).unwrap();
    assert!(first.surveillance.tracks.is_empty());

    let mut confirmation = confirmation_batch();
    set_track(
        &mut confirmation,
        0xabc101,
        FIX_LAT_E7 - NORTH_50M_E7,
        FIX_LON_E7 - EAST_50M_E7,
        EAST_35_MMPS,
        NORTH_35_MMPS,
    );
    let frame = regional_step(&mut engine, &confirmation).unwrap();
    let active = frame
        .aircraft_list
        .iter()
        .find(|view| view.address == 0xabc101)
        .copied()
        .expect("synthetic flight is active after two admitted reports");
    let key = oa_regional_session::route::flight_key(&active);
    let lifecycle_generation = frame.hold_lifecycle_generation(key).unwrap();
    let fix = oa_regional_session::RouteFixInput {
        name: *b"HOLDFIX\0",
        region: *b"K1\0\0",
        latitude_e7: FIX_LAT_E7,
        longitude_e7: FIX_LON_E7,
    };
    let instruction = HoldInstruction {
        flight_key: key,
        fix,
        direction_from_fix_deg: 0,
        inbound_course_deg: 0,
        turn: Some(HoldTurn::Right),
        leg: Some(HoldLegLength {
            value: 1,
            unit: HoldLegUnit::Minutes,
        }),
        efc_utc_minute: None,
        prediction: Some(HoldPredictionIntent {
            entry_procedure: HoldEntryProcedure::Teardrop,
            entry_state_ns: 0,
            course_basis: oa_regional_session::holding::HoldEntryCourseBasis::SyntheticNoWindCourse,
            entry_capture_radius_mm: 500_000,
            inbound_true_course_cdeg: 0,
            leg_duration_ms: 60_000,
            turn_rate_cdeg_s: 300,
            max_scan_gap_ns: 10 * SECOND,
            max_position_error_mm: 1_000_000,
            max_heading_error_cdeg: 4_500,
            max_speed_error_mmps: 10_000,
        }),
    };
    let mut hold_audit = HoldAuditHarness::new("teardrop-hold");
    hold_audit
        .apply(
            &mut engine,
            HoldUpdate::Assign {
                lifecycle_generation,
                revision: 1,
                instruction,
            },
            2 * SECOND,
        )
        .unwrap();
    let mut crossing = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: vec![
            row(
                3 * SECOND,
                5,
                "SURVEILLANCE",
                1,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            ),
            row(
                3 * SECOND,
                6,
                "SURVEILLANCE",
                2,
                0xabc102,
                "POS\t410000000\t-869924000\t10000\t-100000\t0\t0",
            ),
        ],
    };
    set_track(
        &mut crossing,
        0xabc101,
        FIX_LAT_E7 + NORTH_50M_E7,
        FIX_LON_E7 + EAST_50M_E7,
        EAST_35_MMPS,
        NORTH_35_MMPS,
    );
    let current = regional_step(&mut engine, &crossing).unwrap();
    assert_eq!(
        current.hold_phase_profiles_built,
        1,
        "hold={:?}, pair unavailable={:?}, SUA unavailable={:?}, tracks={:?}",
        current.hold_view,
        current.pair_hold_probe_unavailable,
        current.sua_hold_probe_unavailable,
        current.surveillance.tracks
    );
    assert_eq!(current.hold_view.len(), 1);
    let hold = current.hold_view[0];
    let phase = hold.phase_state.unwrap();
    assert_eq!(phase.entry_model, 1);
    assert_eq!(phase.entry_procedure, 3);
    assert_eq!(phase.entry_available_ns, 3 * SECOND);
    assert!(phase.entry_state_ns < phase.entry_available_ns);
    assert_eq!(phase.last_advance_ns, current.surveillance.time_ns);
    let snapshot = hold.phase_snapshot.unwrap();
    assert_eq!(snapshot.at_ns, 3 * SECOND);
    let observed = current
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == 0xabc101)
        .copied()
        .expect("second accepted report supplies the observed track");
    let observed_heading = oa_math::rem_euclid(
        oa_math::atan2(
            f64::from(observed.east_mmps),
            f64::from(observed.north_mmps),
        ) * (180.0 / core::f64::consts::PI),
        360.0,
    );
    let heading_error = oa_math::rem_euclid(
        f64::from(snapshot.heading_true_cdeg) / 100.0 - observed_heading + 180.0,
        360.0,
    ) - 180.0;
    assert!(
        heading_error.abs() <= 45.0,
        "heading continuity: {heading_error}"
    );
    let lat_error_m =
        f64::from(snapshot.lat_e7 - observed.lat_e7) / 10_000_000.0 * 111_194.926_644_558_74;
    let lon_error_m = f64::from(snapshot.lon_e7 - observed.lon_e7) / 10_000_000.0
        * 111_194.926_644_558_74
        * oa_math::cos(41.0_f64.to_radians());
    let position_error_mm = oa_ieee::round(oa_math::hypot(lat_error_m, lon_error_m) * 1_000.0);
    assert!(
        position_error_mm <= 1_000_000.0,
        "position continuity: {position_error_mm} mm"
    );
    assert!(current.pair_hold_probe_unavailable.is_empty());
    assert!(current.sua_hold_probe_unavailable.is_empty());
    assert!(current.pair_probes_unavailable == 0);
    let sua_entry = current
        .sua_probes
        .iter()
        .find(|probe| {
            probe.flight_gufi == 1
                && probe.result.status == oa_airspace::core::conflict_probe::SuaStatus::Entry
        })
        .expect("future teardrop track enters the bounded synthetic SUA");
    assert!(
        (80_000..=100_000).contains(&sua_entry.result.entry_ms),
        "teardrop SUA entry should follow its modeled turn and outbound minute; got {} ms",
        sua_entry.result.entry_ms
    );

    let trial_request = TrialRouteProbeRequest {
        candidate_route: row(
            3 * SECOND,
            7,
            "ROUTE",
            2,
            0xabc102,
            "FILED_ROUTE\tPLAN00000002\tFIXB\tFIX:FIXA:K1:1:- DCT:FIXB:K1:1:-",
        ),
        counterpart_gufi: 1,
    };
    assert!(regional_trial_route_probe(&mut engine, trial_request).is_ok());
}

// REQ: OA-REG-002, OA-REG-003, OA-UI-006
#[test]
fn raw_surveillance_needs_no_invented_flight_identity() {
    let mut engine = runtime();
    let mut latest = None;
    for second in [1, 2] {
        latest = Some(
            regional_step(
                &mut engine,
                &ReceiptBatch {
                    receipt_ns: second * SECOND,
                    source_clock_regressions: 0,
                    events: vec![row(
                        second * SECOND,
                        second as u64,
                        "SURVEILLANCE",
                        0,
                        0xabc101,
                        "POS\t410000000\t-870000000\t10000\t0\t0\t0",
                    )],
                },
            )
            .unwrap(),
        );
    }
    let frame = latest.unwrap();
    assert_eq!(frame.surveillance.tracks.len(), 1);
    assert_eq!(frame.surveillance.displayed, 1);
    assert_eq!(frame.controller_views[0].identity_present, 0);
    assert_eq!(frame.controller_views[0].identity_claims, 0);
    assert!(frame.flights[0].trajectory.is_none());
    assert!(frame.flights[0].refusal.contains("UnknownFlight"));
    assert_eq!(frame.flight_report.flights, 0);
}

#[test]
fn outage_mode_preserves_flight_data_and_recovers_on_a_fresh_surveillance_measurement() {
    let mut engine = runtime();
    let first = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    SECOND,
                    1,
                    "SURVEILLANCE",
                    0,
                    0xabc101,
                    "POS\t410000000\t-870000000\t10000\t0\t0\t0",
                ),
                row(
                    SECOND,
                    1,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tPLAN\tPLAN00000001\t2",
                ),
                row(SECOND, 2, "FLIGHT", 1, 0xabc101, "FLIGHT\tOWNER\tSECTOR1"),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        first.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert!(first.automation.fusion_off);
    assert_eq!(first.flight_report.flights, 1);

    let current = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                2 * SECOND,
                2,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        current.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert!(!current.surveillance.tracks.is_empty());

    let outage = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(3 * SECOND, 3, "SURVEILLANCE", 0, 0, "FEED\tDOWN\t0"),
                row(
                    3 * SECOND,
                    3,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tHO_OFFER\tSECTOR2",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        outage.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        outage.automation.degradation,
        AutomationDegradation::SurveillanceUnavailable
    );
    assert!(outage.surveillance.surveillance_withheld);
    assert!(outage.surveillance.tracks.is_empty());
    assert!(outage.surveillance.withheld_tracks > 0);
    assert_eq!(outage.flight_report.flights, 1);
    assert!(outage.flight_operations[1].operation.is_some());
    assert_eq!(outage.flight_data.len(), 1);
    assert_eq!(outage.flight_data_as_of.len(), outage.flight_data.len());
    assert_eq!(outage.flight_data_unavailable, 0);
    assert_eq!(outage.flight_data[0].handoff_to, *b"SECTOR2\0");
    assert_eq!(
        outage.flight_data_as_of[0].plan,
        FieldAsOf::AsOf {
            source_age_ms: 2_000,
            availability_age_ms: 2_000,
        },
        "a live FLIGHT feed heartbeat cannot make the retained plan look newly updated"
    );

    let recovered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                4,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        recovered.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
    assert_eq!(
        recovered.automation.degradation,
        AutomationDegradation::None
    );
    assert!(!recovered.surveillance.surveillance_withheld);
    assert_eq!(recovered.flight_report.flights, 1);
}

#[test]
fn ca_alert_history_is_retained_with_an_explicit_unavailable_product_status() {
    let mut engine = runtime();
    let active = confirmed(&mut engine);
    assert_eq!(
        active.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Available
    );
    let start_ns = active.surveillance.ca_open[0].start_ns;

    let outage = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(3 * SECOND, 5, "SURVEILLANCE", 0, 0, "FEED\tDOWN\t0")],
        },
    )
    .unwrap();
    assert_eq!(
        outage.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Withheld(
            oa_regional_session::surveillance::ProductUnavailableReason::SurveillanceFeedUnavailable
        )
    );
    assert!(outage.surveillance.withheld_ca_records > 0);
    assert!(outage
        .surveillance
        .ca_open
        .iter()
        .chain(&outage.surveillance.ca_closed)
        .any(|alert| alert.start_ns == start_ns));

    let recovered = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    4 * SECOND,
                    6,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869964000\t10000\t100000\t0\t0",
                ),
                row(
                    4 * SECOND,
                    7,
                    "SURVEILLANCE",
                    2,
                    0xabc102,
                    "POS\t410000000\t-869936000\t10000\t-100000\t0\t0",
                ),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        recovered.surveillance.ca_product,
        oa_regional_session::surveillance::ProductAvailability::Available
    );
}

#[test]
fn unsupported_fusion_selection_is_refused_and_reported_off() {
    let mut engine = runtime();
    assert_eq!(
        regional_select_automation_mode(&mut engine, RegionalAutomationMode::Fusion),
        Err(AutomationModeError::FusionUnavailable)
    );
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        frame.automation.selected,
        RegionalAutomationMode::Surveillance
    );
    assert!(frame.automation.fusion_off);
}

#[test]
fn non_surveillance_feed_state_does_not_claim_aggregate_track_health() {
    let mut engine = runtime();
    let live = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                SECOND,
                1,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        live.automation.effective,
        EffectiveAutomationMode::Surveillance
    );

    let unrelated_feed_loss = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(2 * SECOND, 2, "FLIGHT", 0, 0, "FEED\tDOWN\t0")],
        },
    )
    .unwrap();
    assert_eq!(
        unrelated_feed_loss.automation.effective,
        EffectiveAutomationMode::Surveillance
    );
}

#[test]
fn explicit_flight_data_only_selection_remains_selected_without_sensor_health() {
    let mut engine = runtime();
    regional_select_automation_mode(&mut engine, RegionalAutomationMode::FlightDataOnly).unwrap();
    let first = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                SECOND,
                1,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert_eq!(
        first.automation.selected,
        RegionalAutomationMode::FlightDataOnly
    );
    assert_eq!(
        first.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        first.automation.degradation,
        AutomationDegradation::OperatorSelectedFlightDataOnly
    );
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                2 * SECOND,
                2,
                "SURVEILLANCE",
                0,
                0xabc101,
                "POS\t410000000\t-870000000\t10000\t0\t0\t0",
            )],
        },
    )
    .unwrap();
    assert!(frame.surveillance.surveillance_withheld);
    assert!(frame.surveillance.tracks.is_empty());
    assert!(frame.surveillance.withheld_tracks > 0);
    regional_select_automation_mode(&mut engine, RegionalAutomationMode::Surveillance).unwrap();
    let surveillance_unavailable = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 8 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        surveillance_unavailable.automation.selected,
        RegionalAutomationMode::Surveillance
    );
    assert_eq!(
        surveillance_unavailable.automation.effective,
        EffectiveAutomationMode::FlightDataOnly
    );
    assert_eq!(
        surveillance_unavailable.automation.degradation,
        AutomationDegradation::SurveillanceUnavailable
    );
}

fn row(
    t: i64,
    seq: u64,
    source: &str,
    flight: u128,
    address: u32,
    body: &str,
) -> oa_regional_session::NormalizedEvent {
    parse_event_line(&format!(
        "v1\t{t}\t{t}\t{seq}\t{source}\t{flight:032X}\t{address:06X}\tZAU1\t{body}"
    ))
    .expect("valid synthetic row")
}
fn start() -> ReceiptBatch {
    let mut events = vec![
        row(
            SECOND,
            1,
            "SURVEILLANCE",
            1,
            0xabc101,
            "POS\t410000000\t-870000000\t10000\t100000\t0\t0",
        ),
        row(
            SECOND,
            2,
            "SURVEILLANCE",
            2,
            0xabc102,
            "POS\t410000000\t-869900000\t10000\t-100000\t0\t0",
        ),
    ];
    let mut sequence = 1;
    for (flight, address, plan) in [(1, 0xabc101, "PLAN00000001"), (2, 0xabc102, "PLAN00000002")] {
        for body in [
            format!("FLIGHT\tPLAN\t{plan}\t2"),
            "FLIGHT\tADDRESS".into(),
            "FLIGHT\tASSIGNED\t12000".into(),
            "FLIGHT\tOWNER\tSECTOR1".into(),
        ] {
            events.push(row(SECOND, sequence, "FLIGHT", flight, address, &body));
            sequence += 1;
        }
    }
    sequence = 1;
    for (flight, address, plan) in [(1, 0xabc101, "PLAN00000001"), (2, 0xabc102, "PLAN00000002")] {
        events.push(row(
            SECOND,
            sequence,
            "ROUTE",
            flight,
            address,
            &format!("FILED_ROUTE\t{plan}\tFIXB\t{ROUTE}"),
        ));
        sequence += 1;
        events.push(row(
            SECOND,
            sequence,
            "ROUTE",
            flight,
            address,
            &format!("CURSOR\t{plan}\t{ROUTE_HASH}\tFIX:FIXB:K1:1:-"),
        ));
        sequence += 1;
    }
    ReceiptBatch {
        receipt_ns: SECOND,
        source_clock_regressions: 0,
        events,
    }
}
fn runtime() -> oa_regional_session::runtime::RegionalRuntime {
    runtime_with_model(synthetic_model().unwrap())
}

fn runtime_with_model(
    model: oa_airspace::core::model::Model,
) -> oa_regional_session::runtime::RegionalRuntime {
    let bytes = oa_airspace::core::snapshot::encode(&model).unwrap();
    regional_new(
        model,
        oa_sha256::sha256(&bytes),
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410000000,
            centre_lon_e7: -870000000,
            range_nm: 180.0,
        },
    )
    .unwrap()
}

fn runtime_with_continuous_sua() -> oa_regional_session::runtime::RegionalRuntime {
    runtime_with_sua_bounds(409_500_000, 410_500_000, -870_500_000, -869_500_000)
}

fn runtime_with_sua_bounds(
    south_lat_e7: i32,
    north_lat_e7: i32,
    west_lon_e7: i32,
    east_lon_e7: i32,
) -> oa_regional_session::runtime::RegionalRuntime {
    use oa_airspace::core::model::{
        self as m, boundary_source, edge_interp, level, stratum, via, SegIx, Span, POS_NONE,
    };
    let mut model = synthetic_model().unwrap();
    let facility = intern(&mut model, "KORD");
    let name = intern(&mut model, "HOLD TEST SUA");
    model.boundaries.source.push(boundary_source::CIFP_UR);
    model.boundaries.stratum.push(stratum::SUA_MOA);
    model.boundaries.edge_interp.push(edge_interp::PER_SEGMENT);
    model.boundaries.multiple.push(b'A');
    model.boundaries.facility.push(facility);
    model.boundaries.name.push(name);
    model.boundaries.floor_ft.push(10_000);
    model.boundaries.ceiling_ft.push(20_000);
    model.boundaries.vertices.push(Span {
        start: m::VertexIx::new(0),
        len: 0,
    });
    model.boundaries.segs.push(Span {
        start: SegIx::new(0),
        len: 4,
    });
    model.boundaries.time_code.push(m::time_code::CONTINUOUS);
    model.boundaries.level.push(level::LOW);
    m::bit_push(&mut model.boundaries.floor_agl, false);
    m::bit_push(&mut model.boundaries.ceiling_agl, false);
    m::bit_push(&mut model.boundaries.incomplete, false);
    model.segments.via = vec![via::GEODESIC; 4];
    model.segments.lat = vec![south_lat_e7, south_lat_e7, north_lat_e7, north_lat_e7];
    model.segments.lon = vec![west_lon_e7, east_lon_e7, east_lon_e7, west_lon_e7];
    model.segments.arc_lat = vec![POS_NONE; 4];
    model.segments.arc_lon = vec![POS_NONE; 4];
    model.segments.radius_m = vec![0; 4];
    model.segments.bearing_cdeg = vec![m::NONE_U16; 4];
    runtime_with_model(model)
}

fn intern(
    model: &mut oa_airspace::core::model::Model,
    text: &str,
) -> oa_airspace::core::model::StrIx {
    let raw = model.strings.offsets.len().checked_sub(1).unwrap();
    let index = oa_airspace::core::model::StrIx::new(u32::try_from(raw).unwrap());
    model.strings.text.push_str(text);
    model
        .strings
        .offsets
        .push(u32::try_from(model.strings.text.len()).unwrap());
    index
}

fn insert_bit(bits: &mut oa_airspace::core::model::BitSet, index: usize, value: bool) {
    let old_len = usize::try_from(bits.len).unwrap();
    let old_values: Vec<_> = (0..old_len)
        .map(|row| {
            bits.words
                .get(row / 64)
                .is_some_and(|word| (word >> (row % 64)) & 1 != 0)
        })
        .collect();
    let mut rebuilt = oa_airspace::core::model::BitSet::default();
    for row in 0..=old_len {
        oa_airspace::core::model::bit_push(
            &mut rebuilt,
            if row == index {
                value
            } else {
                old_values[if row < index { row } else { row - 1 }]
            },
        );
    }
    *bits = rebuilt;
}

fn repeated_name_model() -> oa_airspace::core::model::Model {
    let mut model = synthetic_model().unwrap();
    let ident = intern(&mut model, "FIXA");
    let region = intern(&mut model, "K2");
    let points = &mut model.points;
    points
        .kind
        .insert(1, oa_airspace::core::model::point_kind::FIX);
    points.lat.insert(1, 420_000_000);
    points.lon.insert(1, -860_000_000);
    points
        .elev_ft
        .insert(1, oa_airspace::core::model::ELEV_NONE);
    points.ident.insert(1, ident);
    points
        .alt_ident
        .insert(1, oa_airspace::core::model::StrIx::NONE);
    points.region.insert(1, region);
    points
        .scope
        .insert(1, oa_airspace::core::model::StrIx::NONE);
    points.name.insert(1, oa_airspace::core::model::StrIx::NONE);
    for (column, value) in [
        (&mut points.src_nasr, false),
        (&mut points.src_cifp, false),
        (&mut points.in_area, true),
        (&mut points.cls_b, false),
        (&mut points.cls_c, false),
        (&mut points.cls_d, false),
        (&mut points.cls_e, false),
    ] {
        insert_bit(column, 1, value);
    }
    model
}

fn ambiguous_airway_model() -> oa_airspace::core::model::Model {
    use oa_airspace::core::model as m;

    let mut model = synthetic_model().unwrap();
    let fix_c = intern(&mut model, "FIXC");
    let fix_d = intern(&mut model, "FIXD");
    let region = intern(&mut model, "K1");
    let airway = intern(&mut model, "V1");
    for (lat, lon, ident) in [
        (415_000_000, -865_000_000, fix_c),
        (405_000_000, -855_000_000, fix_d),
    ] {
        let points = &mut model.points;
        points.kind.push(m::point_kind::FIX);
        points.lat.push(lat);
        points.lon.push(lon);
        points.elev_ft.push(m::ELEV_NONE);
        points.ident.push(ident);
        points.alt_ident.push(m::StrIx::NONE);
        points.region.push(region);
        points.scope.push(m::StrIx::NONE);
        points.name.push(m::StrIx::NONE);
        for (column, value) in [
            (&mut points.src_nasr, false),
            (&mut points.src_cifp, false),
            (&mut points.in_area, true),
            (&mut points.cls_b, false),
            (&mut points.cls_c, false),
            (&mut points.cls_d, false),
            (&mut points.cls_e, false),
        ] {
            m::bit_push(column, value);
        }
    }
    model.airways.designator.push(airway);
    model.airways.location.push(b'C');
    model.airways.legs.push(m::Span {
        start: m::AirwayLegIx::new(0),
        len: 4,
    });
    for (from, to) in [(0, 2), (2, 1), (0, 3), (3, 1)] {
        let legs = &mut model.airway_legs;
        legs.from.push(m::PointIx::new(from));
        legs.to.push(m::PointIx::new(to));
        legs.mea_ft.push(m::ALT_NONE);
        legs.mea_opp_ft.push(m::ALT_NONE);
        legs.gps_mea_ft.push(m::ALT_NONE);
        legs.mea_dir.push(m::compass::NONE);
        legs.gps_mea_dir.push(m::compass::NONE);
        m::bit_push(&mut legs.gap, false);
        m::bit_push(&mut legs.signal_gap, false);
        m::bit_push(&mut legs.dogleg, false);
    }
    model
}

fn confirmed(
    engine: &mut oa_regional_session::runtime::RegionalRuntime,
) -> oa_regional_session::runtime::RegionalFrame {
    let tentative = regional_step(engine, &start()).unwrap();
    assert!(
        tentative.surveillance.tracks.is_empty(),
        "one report is intentionally tentative"
    );
    regional_step(engine, &confirmation_batch()).unwrap()
}

fn confirmation_batch() -> ReceiptBatch {
    ReceiptBatch {
        receipt_ns: 2 * SECOND,
        source_clock_regressions: 0,
        events: vec![
            row(
                2 * SECOND,
                3,
                "SURVEILLANCE",
                1,
                0xabc101,
                "POS\t410000000\t-869988000\t10000\t100000\t0\t0",
            ),
            row(
                2 * SECOND,
                4,
                "SURVEILLANCE",
                2,
                0xabc102,
                "POS\t410000000\t-869912000\t10000\t-100000\t0\t0",
            ),
        ],
    }
}

fn set_position_altitude(batch: &mut ReceiptBatch, altitude_ft: i32) {
    for event in &mut batch.events {
        if let oa_regional_session::EventKind::Position {
            altitude_ft: field, ..
        } = &mut event.kind
        {
            *field = Some(altitude_ft);
        }
    }
}

// REQ: OA-REG-015
#[test]
fn selection_and_console_replace_only_controls_and_keep_the_observation_epoch() {
    use oa_regional_session::runtime::{
        regional_console_overlay, regional_pixels, regional_select,
    };
    let mut engine = runtime();
    let frame = confirmed(&mut engine);
    let view = frame
        .controller_views
        .iter()
        .find(|view| view.address == 0xabc101)
        .unwrap();
    let selected = regional_select(
        &mut engine,
        view.address,
        view.generation,
        Some(view.identity),
    )
    .unwrap();
    for _ in 0..100 {
        let edited = regional_console_overlay(&mut engine, "INPUT ASSIGNED 12000").unwrap();
        assert_ne!(edited, selected);
        assert_eq!(regional_console_overlay(&mut engine, "").unwrap(), selected);
    }
    let before = regional_pixels(&engine).unwrap().to_vec();
    assert!(regional_console_overlay(&mut engine, "bad\ninput").is_err());
    assert_eq!(regional_pixels(&engine).unwrap(), before);
    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(next.controller_selected_address, Some(view.address));
    assert_eq!(next.controller_selection, Some(view.identity));
    assert_eq!(next.surveillance.time_ns, 3 * SECOND);
    assert_eq!(next.surveillance.ca_stats.tracks, 2);
    assert!(regional_select(
        &mut engine,
        view.address,
        view.generation + 1,
        Some(view.identity)
    )
    .is_err());
    assert!(
        regional_pixels(&engine).is_ok(),
        "ordinary selection refusal remains visible"
    );
    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(next.controller_selected_address, None);
    assert_eq!(next.controller_selection, None);
}

// REQ: OA-REG-004
#[test]
fn actual_tracker_flight_route_ca_and_scope_share_one_cutoff() {
    let frame = confirmed(&mut runtime());
    assert_eq!(frame.surveillance.tracks.len(), 2);
    assert_eq!(frame.surveillance.displayed, 2);
    assert_eq!(frame.surveillance.time_ns, 2 * SECOND);
    assert_eq!(frame.flights.len(), 2);
    for flight in &frame.flights {
        assert!(flight.refusal.is_empty(), "{}", flight.refusal);
        let view = flight.view.unwrap();
        assert_eq!(view.address, flight.address);
        let forecast = flight.trajectory.unwrap();
        assert_eq!(forecast.key.aircraft_address, flight.address);
        assert_eq!(forecast.cutoff_ms, 2000);
        assert_eq!(
            forecast.horizons[0].altitude_ft, 10000,
            "a level observation must not teleport to its clearance"
        );
        assert_ne!(forecast.horizons[0].lat_e7, i32::MIN);
    }
    assert!(
        !frame.surveillance.ca_open.is_empty(),
        "two closing aircraft are already inside development minima"
    );
}

// REQ: OA-REG-018
#[test]
fn la2014_extreme_altitude_survives_malformed_route_quarantine() {
    let mut engine = runtime();
    let mut initial = start();
    set_position_altitude(&mut initial, 100_000);
    let tentative = regional_step(&mut engine, &initial).unwrap();
    assert!(tentative.surveillance.tracks.is_empty());

    let mut confirmation = confirmation_batch();
    set_position_altitude(&mut confirmation, 100_000);
    let retained = regional_step(&mut engine, &confirmation).unwrap();
    assert!(retained.surveillance.displayed > 0);
    let high_track = retained
        .surveillance
        .tracks
        .iter()
        .find(|track| track.address == 0xabc101)
        .unwrap();
    assert_eq!(high_track.altitude_ft, 100_000);

    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tTHIS IS NOT A ROUTE",
                ),
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t100000\t100000\t0\t0",
                ),
            ],
        },
    )
    .expect("a known malformed route item must not discard its healthy neighbor");

    assert_eq!(frame.quarantine.records.len(), 1);
    let refused = &frame.quarantine.records[0];
    assert_eq!(
        refused.identity.source,
        oa_quarantine::InputSource::FlightData
    );
    assert_eq!(refused.identity.source_instance, 1);
    assert_eq!(refused.identity.item_id, 3_u64 << 8);
    assert_eq!(
        refused.reasons.0,
        oa_quarantine::REASON_ALTITUDE_ROUTE_INVALID
    );
    assert_eq!(frame.input.input_events, 2);
    assert_eq!(frame.input.source_events[0], 1);
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);
    let later = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                6,
                "ROUTE",
                1,
                0xabc101,
                &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
            )],
        },
    )
    .expect("later valid route revision should restore availability for its cutoff");
    assert_eq!(later.quarantine.records.len(), 0);
    assert_eq!(later.quarantine.source_stats[0].queued, 1);
    assert_eq!(later.quarantine.source_stats[0].accepted, 13);
}

// REQ: OA-REG-018
#[test]
fn nats2023_repeated_local_name_resolves_with_full_region_identity_at_runtime() {
    let mut engine = runtime_with_model(repeated_name_model());
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t65000\t100000\t0\t0",
                ),
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tFIX:FIXA:K2:1:- FIX:FIXB:K1:1:-",
                ),
            ],
        },
    )
    .expect("full route identity and high but supported altitude remain admissible");

    assert_eq!(frame.quarantine.records.len(), 0);
    assert_eq!(frame.input.source_events, [1, 0, 1, 0, 0]);
    assert_eq!(frame.quarantine.source_stats[0].accepted, 13);
    assert_eq!(frame.quarantine.source_stats[0].queued, 0);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);
}

// REQ: OA-REG-018
#[test]
fn ambiguous_route_is_quarantined_and_following_healthy_item_commits() {
    let mut engine = runtime_with_model(ambiguous_airway_model());
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    5,
                    "ROUTE",
                    1,
                    0xabc101,
                    "FILED_ROUTE\tPLAN00000001\tFIXB\tAWY:V1:C:FIXA:K1:1:-:FIXB:K1:1:-:12000",
                ),
                row(
                    3 * SECOND,
                    5,
                    "SURVEILLANCE",
                    1,
                    0xabc101,
                    "POS\t410000000\t-869988000\t10000\t100000\t0\t0",
                ),
            ],
        },
    )
    .expect("ambiguous route is isolated and the next normalized item is handled");

    assert_eq!(frame.quarantine.records.len(), 1);
    assert_eq!(
        frame.quarantine.records[0].reasons.0,
        oa_quarantine::REASON_WAYPOINT_NAME_AMBIGUOUS
    );
    assert_eq!(frame.quarantine.records[0].identity.item_id, 3_u64 << 8);
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[1].accepted, 5);
    assert_eq!(frame.quarantine.source_stats[1].queued, 0);

    let later = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 4 * SECOND,
            source_clock_regressions: 0,
            events: vec![row(
                4 * SECOND,
                6,
                "ROUTE",
                1,
                0xabc101,
                &format!("FILED_ROUTE\tPLAN00000001\tFIXB\t{ROUTE}"),
            )],
        },
    )
    .expect("a later valid amendment remains acceptable after the fenced refusal");
    assert_eq!(later.quarantine.records.len(), 0);
    assert_eq!(later.quarantine.source_stats[0].queued, 1);
    assert_eq!(later.quarantine.source_stats[0].accepted, 13);
}

// REQ: OA-REG-018
#[test]
fn staged_flight_adapter_refusal_does_not_advance_or_block_following_operation() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![
                row(
                    3 * SECOND,
                    9,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tOWNER\tSECTOR2",
                ),
                row(
                    3 * SECOND,
                    10,
                    "FLIGHT",
                    1,
                    0xabc101,
                    "FLIGHT\tASSIGNED\t13000",
                ),
            ],
        },
    )
    .expect("a staged adapter refusal must permit a following valid operation");

    assert_eq!(frame.quarantine.records.len(), 1);
    assert_eq!(frame.quarantine.records[0].identity.item_id, 3_u64 << 8);
    assert_eq!(
        frame.quarantine.records[0].reasons.0,
        oa_quarantine::REASON_INPUT_INVALID
    );
    assert_eq!(frame.quarantine.source_stats[0].queued, 1);
    assert_eq!(frame.quarantine.source_stats[0].accepted, 13);
    assert_eq!(frame.flight_report.adapter.owner_change_requires_take, 1);
    let flight = frame
        .flights
        .iter()
        .find(|flight| flight.address == 0xabc101)
        .unwrap();
    let view = flight.view.unwrap();
    assert_eq!(view.owner_unit, *b"SECTOR1\0");
    assert_eq!(view.assigned.form, oa_flight::ALT_SIMPLE);
    assert_eq!(view.assigned.a_ft, 13_000);
}

// REQ: OA-REG-005
#[test]
fn replayed_prefix_is_identical_and_future_amendments_do_not_change_it() {
    let mut first = runtime();
    let before = confirmed(&mut first);
    let amendment = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            3 * SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tUNASSIGNED",
        )],
    };
    let later = regional_step(&mut first, &amendment).unwrap();
    assert_eq!(
        later.flights[0].view.unwrap().assigned.form,
        oa_flight::ALT_NIL
    );
    let independently_reconstructed = confirmed(&mut runtime());
    assert_eq!(before, independently_reconstructed);
}

// REQ: OA-REG-006
#[test]
fn route_plan_mismatch_blocks_old_route_and_failed_frame_requires_reconstruction() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let changed = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            3 * SECOND,
            9,
            "FLIGHT",
            1,
            0xabc101,
            "FLIGHT\tPLAN\tPLAN00000003\t2",
        )],
    };
    let frame = regional_step(&mut engine, &changed).unwrap();
    assert!(frame.flights[0].trajectory.is_none());
    assert!(
        frame.flights[0].refusal.contains("NoAvailableEvidence"),
        "route plan change must refuse without matching full-key route evidence: {}",
        frame.flights[0].refusal
    );
    let invalid = ReceiptBatch {
        receipt_ns: 3 * SECOND,
        source_clock_regressions: 0,
        events: Vec::new(),
    };
    assert!(regional_step(&mut engine, &invalid).is_err());
    let next = ReceiptBatch {
        receipt_ns: 4 * SECOND,
        ..invalid
    };
    assert!(regional_step(&mut engine, &next)
        .unwrap_err()
        .contains("reconstruct"));
}

// REQ: OA-REG-007
#[test]
fn feed_loss_is_visible_without_erasing_or_reidentifying_traffic() {
    let mut engine = runtime();
    confirmed(&mut engine);
    let loss = ReceiptBatch {
        receipt_ns: 6 * SECOND,
        source_clock_regressions: 0,
        events: vec![row(
            6 * SECOND,
            5,
            "SURVEILLANCE",
            1,
            0xabc101,
            "FEED\tDOWN\t1",
        )],
    };
    let frame = regional_step(&mut engine, &loss).unwrap();
    assert!(frame.surveillance.surveillance_withheld);
    assert!(frame.surveillance.tracks.is_empty());
    assert!(frame.surveillance.withheld_tracks >= 2);
    assert_eq!(frame.flight_data.len(), 2, "flight data survives feed loss");
    assert!(
        frame
            .surveillance
            .warnings
            .iter()
            .any(|text| text.contains("TRACK") && text.contains("LOST")),
        "{:?}",
        frame.surveillance.warnings
    );
}

fn first_flight_key() -> FlightKey {
    FlightKey {
        gufi: 1u128.to_be_bytes(),
        aircraft_address: 0xabc101,
        centre: *b"ZAU1",
        plan_id: *b"PLAN00000001",
    }
}

fn performance_for(key: FlightKey) -> PerformanceEnvelope {
    PerformanceEnvelope {
        key,
        reserved_key_align: [0; 4],
        source_ms: 1_000,
        available_ms: 2_000,
        message_id: 9001,
        max_age_ms: 5_000,
        max_climb_fpm: 600,
        max_descent_fpm: 600,
        max_groundspeed_mmps: 1_000_000,
        reserved_speed_cap: 0,
        source: PerformanceSource::Measured,
        status: PerformanceStatus::Valid,
        reserved: [0; 6],
    }
}

fn confirmed_with_performance(
    performance: &[PerformanceEnvelope],
) -> oa_regional_session::runtime::RegionalFrame {
    let mut engine = runtime();
    regional_step(&mut engine, &start()).unwrap();
    regional_step_with_performance(&mut engine, &confirmation_batch(), performance).unwrap()
}

// REQ: OA-REG-014
#[test]
fn opt_in_uses_full_key_performance_and_preserves_horizontal_only_results() {
    let baseline = confirmed(&mut runtime());
    for flight in &baseline.flights {
        assert!(flight.vertical_profile.is_none());
        assert_eq!(flight.vertical_refusal, "vertical profile not requested");
    }

    let envelope = performance_for(first_flight_key());
    let opt_in = confirmed_with_performance(&[envelope]);
    let baseline_first = baseline
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    let opt_in_first = opt_in
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    assert_eq!(baseline_first.trajectory, opt_in_first.trajectory);
    let profile = opt_in_first.vertical_profile.unwrap();
    assert_eq!(profile.performance, envelope);
    assert_eq!(profile.key, envelope.key);
    assert_eq!(
        profile.availability,
        oa_legacy_traj::core::trajectory::Availability::Available
    );
    assert_eq!(
        profile.points[0].availability,
        oa_legacy_traj::core::trajectory::Availability::Available
    );
    assert_eq!(profile.points[0].reachable_min_ft, 10_000);
    assert!(profile.points[0].reachable_max_ft >= 10_000);
    assert!(opt_in_first.vertical_refusal.is_empty());

    let other_flight = opt_in
        .flights
        .iter()
        .find(|f| f.address == 0xabc102)
        .unwrap();
    assert!(other_flight.vertical_profile.is_none());
    assert!(other_flight.vertical_refusal.contains("full flight key"));

    let disabled = confirmed_with_performance(&[]);
    for expected in &baseline.flights {
        let actual = disabled
            .flights
            .iter()
            .find(|f| f.address == expected.address)
            .unwrap();
        assert_eq!(expected.trajectory, actual.trajectory);
        assert!(actual.vertical_profile.is_none());
    }
}

// REQ: OA-REG-014
#[test]
fn only_one_eligible_exact_full_key_envelope_is_selected() {
    let mut mismatched_key = first_flight_key();
    mismatched_key.plan_id[0] ^= 1;
    let mismatch_frame = confirmed_with_performance(&[performance_for(mismatched_key)]);
    let mismatch = mismatch_frame
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    assert!(mismatch.trajectory.is_some());
    assert!(mismatch.vertical_profile.is_none());
    assert!(mismatch.vertical_refusal.contains("full flight key"));

    let envelope = performance_for(first_flight_key());
    let duplicate_frame = confirmed_with_performance(&[envelope, envelope]);
    let duplicate = duplicate_frame
        .flights
        .iter()
        .find(|f| f.address == 0xabc101)
        .unwrap();
    assert!(duplicate.trajectory.is_some());
    assert!(duplicate.vertical_profile.is_none());
    assert!(duplicate.vertical_refusal.contains("duplicate eligible"));
}

// REQ: OA-REG-014
#[test]
fn future_performance_payload_does_not_change_the_eligible_profile_or_baseline() {
    let eligible = performance_for(first_flight_key());
    let expected = confirmed_with_performance(&[eligible]);

    let mut future = eligible;
    future.source_ms = 2_001;
    future.available_ms = 2_002;
    future.key.plan_id = [b'X'; 12];
    future.max_climb_fpm = u32::MAX;
    future.max_descent_fpm = u32::MAX;
    future.max_groundspeed_mmps = u32::MAX;
    future.reserved_key_align = [1; 4];
    future.reserved = [1; 6];
    future.status = PerformanceStatus::Contradictory;
    let actual = confirmed_with_performance(&[eligible, future]);
    assert_eq!(actual, expected);
}

// REQ: OA-REG-014
#[test]
fn over_capacity_performance_slice_refuses_before_runtime_mutation() {
    let mut engine = runtime();
    let envelope = performance_for(first_flight_key());
    let rows = vec![envelope; MAX_PERFORMANCE_ENVELOPES + 1];
    let error = regional_step_with_performance(&mut engine, &start(), &rows).unwrap_err();
    assert!(error.contains("performance envelope slice exceeds"));

    let frame = regional_step(&mut engine, &start()).unwrap();
    assert_eq!(frame.sequence, 1);
    assert!(frame.flights.is_empty());
}

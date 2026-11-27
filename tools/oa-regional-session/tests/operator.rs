#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use oa_display::interaction::{FlightIdentity, Intent};
use oa_regional_session::input::{
    EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind,
};
use oa_regional_session::operator::{
    operator_journal_bytes, operator_journal_new, parse_action, parse_actions, prepare_batch,
    run_script_session_with_typed_operator_inputs, Action, OperatorAuditConfiguration,
    RecordedBatch, RecordedEvent, TypedOperatorInput,
};
use oa_regional_session::session::MAX_EVENTS_PER_RECEIPT;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

fn temp_path(label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "oa-regional-operator-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn synthetic_complete_runway_profile() -> Vec<u8> {
    let mut bytes = vec![
        0;
        oa_runway::adaptation::HEADER_V2_BYTES
            + oa_runway::adaptation::PAIR_BYTES
            + 2 * oa_runway::adaptation::PATH_BYTES
    ];
    bytes[..8].copy_from_slice(oa_runway::adaptation::MAGIC_V2);
    bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
    bytes[10..12].copy_from_slice(&2u16.to_le_bytes());
    bytes[12] = oa_runway::adaptation::COVERAGE_COMPLETE_AIRPORT;
    let pair = &mut bytes[oa_runway::adaptation::HEADER_V2_BYTES
        ..oa_runway::adaptation::HEADER_V2_BYTES + oa_runway::adaptation::PAIR_BYTES];
    pair[..4].copy_from_slice(b"KORD");
    pair[4..8].copy_from_slice(&1u32.to_le_bytes());
    pair[8..11].copy_from_slice(b"09L");
    pair[11..13].copy_from_slice(&150u16.to_le_bytes());
    pair[13..17].copy_from_slice(&2u32.to_le_bytes());
    pair[17..20].copy_from_slice(b"09R");
    pair[20..22].copy_from_slice(&150u16.to_le_bytes());
    pair[22..24].copy_from_slice(&800u16.to_le_bytes());
    pair[24..26].copy_from_slice(&1_000u16.to_le_bytes());
    pair[26..30].copy_from_slice(&10_000u32.to_le_bytes());
    pair[30..34].copy_from_slice(&4_800u32.to_le_bytes());
    pair[34..38].copy_from_slice(&10_000u32.to_le_bytes());
    pair[38..42].copy_from_slice(&200_000u32.to_le_bytes());
    pair[42..44].copy_from_slice(&500u16.to_le_bytes());

    let paths_start = oa_runway::adaptation::HEADER_V2_BYTES + oa_runway::adaptation::PAIR_BYTES;
    for (index, (path_id, name, longitude)) in [
        (1u32, *b"09L ", -870_060_000i32),
        (2, *b"09R ", -869_940_000),
    ]
    .into_iter()
    .enumerate()
    {
        let start = paths_start + index * oa_runway::adaptation::PATH_BYTES;
        let path = &mut bytes[start..start + oa_runway::adaptation::PATH_BYTES];
        path[..4].copy_from_slice(b"KORD");
        path[4..8].copy_from_slice(&path_id.to_le_bytes());
        path[8] = oa_runway::adaptation::PATH_KIND_RUNWAY;
        path[9..13].copy_from_slice(&name);
        path[13..17].copy_from_slice(&45_720u32.to_le_bytes());
        path[17..21].copy_from_slice(&410_044_915i32.to_le_bytes());
        path[21..25].copy_from_slice(&longitude.to_le_bytes());
        path[25..29].copy_from_slice(&409_955_085i32.to_le_bytes());
        path[29..33].copy_from_slice(&longitude.to_le_bytes());
    }
    bytes
}

fn synthetic_runway_model() -> oa_airspace::core::model::Model {
    use oa_airspace::core::model::{self, Meta};

    let mut model = model::empty_model(Meta {
        nasr_effective: 20261101,
        cifp_cycle: 2611,
        artcc: "ZAU".to_owned(),
        buffer_m: 0,
        nasr_sha256: [1; 32],
        cifp_sha256: [2; 32],
    });
    let rows = [
        (
            "09L",
            "KORD",
            model::point_kind::THRESHOLD,
            410_044_915,
            -870_060_000,
            false,
        ),
        (
            "09R",
            "KORD",
            model::point_kind::THRESHOLD,
            410_044_915,
            -869_940_000,
            false,
        ),
        (
            "27L",
            "KORD",
            model::point_kind::THRESHOLD,
            409_955_085,
            -869_940_000,
            false,
        ),
        (
            "27R",
            "KORD",
            model::point_kind::THRESHOLD,
            409_955_085,
            -870_060_000,
            false,
        ),
        (
            "KORD",
            "",
            model::point_kind::AIRPORT,
            410_000_000,
            -870_000_000,
            true,
        ),
    ];
    for (ident, scope, kind, lat, lon, airport) in rows {
        let intern = |model: &mut model::Model, text: &str| {
            let raw = model.strings.offsets.len().saturating_sub(1);
            let ix = model::StrIx::new(u32::try_from(raw).unwrap());
            model.strings.text.push_str(text);
            model
                .strings
                .offsets
                .push(u32::try_from(model.strings.text.len()).unwrap());
            ix
        };
        let ident_ix = intern(&mut model, ident);
        let region_ix = intern(&mut model, "K1");
        let scope_ix = if scope.is_empty() {
            model::StrIx::NONE
        } else {
            intern(&mut model, scope)
        };
        model.points.kind.push(kind);
        model.points.lat.push(lat);
        model.points.lon.push(lon);
        model.points.elev_ft.push(model::ELEV_NONE);
        model.points.ident.push(ident_ix);
        model.points.alt_ident.push(model::StrIx::NONE);
        model.points.region.push(region_ix);
        model.points.scope.push(scope_ix);
        model.points.name.push(model::StrIx::NONE);
        model::bit_push(&mut model.points.src_nasr, airport);
        model::bit_push(&mut model.points.src_cifp, !airport);
        model::bit_push(&mut model.points.in_area, true);
        model::bit_push(&mut model.points.cls_b, false);
        model::bit_push(&mut model.points.cls_c, false);
        model::bit_push(&mut model.points.cls_d, false);
        model::bit_push(&mut model.points.cls_e, false);
    }
    model
}

fn issue_synthetic_runway_package(root: &std::path::Path) -> ([u8; 32], PathBuf) {
    use oa_adapt::manifest::{
        issue_new_package_with_pver_catalog, issue_new_package_with_runway_aids, verify_current,
        write_new_package, ChangeClass,
    };

    let model = synthetic_runway_model();
    let snapshot = oa_airspace::core::snapshot::encode(&model).unwrap();
    let previous_root = root.join("previous");
    let previous_pin = write_new_package(&previous_root, &snapshot, "base", "ZAU", "base").unwrap();
    let pver = include_bytes!("../../oa-certification/data/synthetic-pver-v1.csv");
    let v4_root = root.join("v4");
    let (_, v4_pin) = issue_new_package_with_pver_catalog(
        &v4_root,
        &snapshot,
        pver,
        None,
        "pver",
        "ZAU",
        "synthetic-pver",
        &previous_root,
        previous_pin,
        ChangeClass::Initial,
        "issuer",
        "approver",
        "synthetic PVER fixture",
    )
    .unwrap();
    let package_root = root.join("issued-runway");
    let runway_profile = synthetic_complete_runway_profile();
    let (_, pin) = issue_new_package_with_runway_aids(
        &package_root,
        &snapshot,
        &runway_profile,
        "runway",
        "ZAU",
        "synthetic-runway",
        &v4_root,
        v4_pin,
        ChangeClass::Routine,
        "issuer",
        "approver",
        "synthetic runway fixture",
    )
    .unwrap();
    let verified = verify_current(&package_root, pin).unwrap();
    assert_eq!(
        verified.runway_aids_bytes(),
        Some(runway_profile.as_slice())
    );
    (pin, package_root)
}

fn synthetic_runway_session() -> String {
    let mut text = String::from("OA-RS\t1\n");
    for scan in 1..=9u64 {
        let time = scan * 1_000_000_000;
        for (sequence, gufi, address, longitude_offset) in [
            (
                scan * 2 - 1,
                "00000000000000000000000000000001",
                "ABC101",
                60_000i32,
            ),
            (
                scan * 2,
                "00000000000000000000000000000002",
                "ABC102",
                -60_000i32,
            ),
        ] {
            let latitude = 410_000_000 + i32::try_from((scan - 1) * 900).unwrap();
            let longitude = -870_000_000 + longitude_offset;
            text.push_str(&format!(
                "v1\t{time}\t{time}\t{sequence}\tSURVEILLANCE\t{gufi}\t{address}\tZAU1\tPOS\t{latitude}\t{longitude}\t10000\t0\t10000\t0\n"
            ));
        }
        if scan == 1 {
            text.push_str(&format!(
                "v1\t{time}\t{time}\t1\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tPLAN\tPLAN00000001\t2\n"
            ));
            text.push_str(&format!(
                "v1\t{time}\t{time}\t2\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tADDRESS\n"
            ));
            text.push_str(&format!(
                "v1\t{time}\t{time}\t3\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tASSIGNED\t12000\n"
            ));
            text.push_str(&format!(
                "v1\t{time}\t{time}\t4\tFLIGHT\t00000000000000000000000000000001\tABC101\tZAU1\tFLIGHT\tOWNER\tSECTOR1\n"
            ));
        }
    }
    text
}

fn console_binaries() -> &'static [PathBuf; 2] {
    static BINARIES: OnceLock<[PathBuf; 2]> = OnceLock::new();
    BINARIES.get_or_init(|| {
        // Cargo's artifact has hard-link aliases, which the executable seal guard refuses.
        // Finish both private copies before either console can inherit another copy's write
        // descriptor across fork/exec (ETXTBSY). Only fixture setup is shared; tests run in parallel.
        let binaries = [temp_path("console-bin"), temp_path("full-bin")];
        for path in &binaries {
            fs::copy(env!("CARGO_BIN_EXE_oa-regional-console"), path).unwrap();
        }
        binaries
    })
}

fn verify_closed_bundle(output: &std::path::Path) {
    let manifest_bytes = fs::read(output.join("manifest.tsv")).unwrap();
    let manifest = std::str::from_utf8(&manifest_bytes).unwrap();
    let complete = fs::read_to_string(output.join("COMPLETE")).unwrap();
    assert_eq!(
        complete.trim(),
        oa_sha256::to_hex(&oa_sha256::sha256(&manifest_bytes))
    );
    let mut listed = std::collections::BTreeSet::new();
    for line in manifest.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["settings", name, digest]
            | ["operator_journal", name, digest]
            | ["processor_report", name, digest] => {
                let bytes = fs::read(output.join(*name)).unwrap();
                assert_eq!(*digest, oa_sha256::to_hex(&oa_sha256::sha256(&bytes)));
                listed.insert((*name).to_owned());
            }
            ["frame", _, _, report_name, report_hash, ppm_name, ppm_hash] => {
                let report = fs::read(output.join(*report_name)).unwrap();
                let ppm = fs::read(output.join(*ppm_name)).unwrap();
                assert_eq!(*report_hash, oa_sha256::to_hex(&oa_sha256::sha256(&report)));
                assert_eq!(*ppm_hash, oa_sha256::to_hex(&oa_sha256::sha256(&ppm)));
                let report_text = std::str::from_utf8(&report).unwrap();
                assert!(report_text.contains(&format!("pixels_ppm_sha256\t{ppm_hash}")));
                assert!(report_text.contains("rendered_frame_sha256\t"));
                listed.insert((*report_name).to_owned());
                listed.insert((*ppm_name).to_owned());
            }
            _ => {}
        }
    }
    assert!(listed.contains("settings.tsv"));
    assert!(listed.contains("operator.tsv"));
    assert!(listed.contains("processor-report.tsv"));
    let actual: std::collections::BTreeSet<_> = fs::read_dir(output)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "manifest.tsv" && name != "COMPLETE")
        .collect();
    assert_eq!(actual, listed);
}

// REQ: OA-REG-016
#[test]
fn grammar_is_explicit_and_bounded() {
    assert_eq!(
        parse_action("SELECT ABC101"),
        Ok(Action::Select(0x00ab_c101))
    );
    assert_eq!(parse_action("ACTOR 2"), Ok(Action::Actor(2)));
    assert!(parse_action("SELECT 0").is_err());
    assert!(parse_action("ASSIGNED 12000.5").is_err());
    assert!(parse_action("NEXT trailing").is_err());
    assert!(parse_action(&"N".repeat(129)).is_err());
    assert!(parse_actions(b"NEXT\nQUIT\nNEXT\n").is_err());
}

// REQ: OA-REG-016
#[test]
fn local_events_are_inserted_before_route_and_sequences_are_audited() {
    let identity_input = Identity {
        flight_gufi: 1,
        aircraft_address: 0x00ab_c101,
    };
    let recorded = RecordedBatch {
        receipt_ns: 1_000_000_000,
        source_clock_regressions: 0,
        events: vec![
            RecordedEvent {
                event: NormalizedEvent {
                    receipt_ns: 1_000_000_000,
                    source_ns: 1_000_000_000,
                    sequence: 9,
                    source: SourceKind::Flight,
                    identity: identity_input,
                    centre: *b"ZAU1",
                    kind: EventKind::Flight(FlightOperation::ClearAssignedAltitude),
                },
                original_sequence: 9,
                ordinal: 1,
            },
            RecordedEvent {
                event: NormalizedEvent {
                    receipt_ns: 1_000_000_000,
                    source_ns: 1_000_000_000,
                    sequence: 7,
                    source: SourceKind::Route,
                    identity: identity_input,
                    centre: *b"ZAU1",
                    kind: EventKind::FeedState {
                        available: true,
                        reason: 0,
                    },
                },
                original_sequence: 7,
                ordinal: 2,
            },
        ],
    };
    let identity = FlightIdentity {
        gufi: [1; 16],
        aircraft_address: 0x00ab_c101,
        centre: *b"ZAU1",
        plan_id: *b"PLAN00000001",
    };
    let selected = oa_regional_session::operator::SelectedTarget {
        view: oa_display::interaction::TargetView {
            address: identity.aircraft_address,
            generation: 3,
            identity_claims: 1,
            identity_present: 1,
            reserved: 0,
            identity,
        },
        identity,
        view_sequence: 4,
    };
    let command = oa_regional_session::operator::make_command(
        selected,
        1,
        Intent::SetAssignedAltitude(15_000),
        1,
    )
    .unwrap();
    let mut sequences = [0; 5];
    let mut journal = operator_journal_new();
    let prepared = prepare_batch(&recorded, &[command], &mut sequences, &mut journal).unwrap();
    assert_eq!(prepared.events.len(), 3);
    assert_eq!(prepared.events[0].source, SourceKind::Flight);
    assert!(matches!(prepared.events[1].kind, EventKind::Operator(_)));
    assert_eq!(prepared.events[2].source, SourceKind::Route);
    assert_eq!(prepared.events[0].sequence, 1);
    assert_eq!(prepared.events[1].sequence, 2);
    assert_eq!(prepared.events[2].sequence, 1);
    assert!(std::str::from_utf8(operator_journal_bytes(&journal))
        .unwrap()
        .contains("RESEQUENCE\t"));
    assert!(std::str::from_utf8(operator_journal_bytes(&journal))
        .unwrap()
        .contains("LOCAL\t1\tFLIGHT"));
    assert!(prepare_batch(
        &recorded,
        &vec![command; MAX_EVENTS_PER_RECEIPT],
        &mut sequences,
        &mut journal
    )
    .is_err());
}

// REQ: OA-REG-016
#[test]
fn explicit_selection_refuses_ambiguous_or_unbound_address() {
    let identity = FlightIdentity {
        gufi: [1; 16],
        aircraft_address: 0x00ab_c101,
        centre: *b"ZAU1",
        plan_id: *b"PLAN00000001",
    };
    let view = oa_display::interaction::TargetView {
        address: identity.aircraft_address,
        generation: 2,
        identity_claims: 1,
        identity_present: 1,
        reserved: 0,
        identity,
    };
    assert!(oa_regional_session::operator::selected_target(&[view], view.address, 3).is_ok());
    let duplicate = oa_display::interaction::TargetView {
        generation: 3,
        ..view
    };
    assert!(
        oa_regional_session::operator::selected_target(&[view, duplicate], view.address, 3)
            .is_err()
    );
    let raw = oa_display::interaction::TargetView {
        identity_present: 0,
        ..view
    };
    assert!(oa_regional_session::operator::selected_target(&[raw], view.address, 3).is_err());
    assert!(
        oa_regional_session::operator::selected_target(&vec![view; 33], view.address, 3)
            .unwrap_err()
            .contains("work budget")
    );
}

// REQ: OA-REG-016
#[test]
fn address_reuse_cannot_retarget_a_selected_controller_command() {
    let first_identity = FlightIdentity {
        gufi: [1; 16],
        aircraft_address: 0x00ab_c101,
        centre: *b"ZAU1",
        plan_id: *b"PLAN00000001",
    };
    let replacement_identity = FlightIdentity {
        gufi: [2; 16],
        plan_id: *b"PLAN00000002",
        ..first_identity
    };
    let first = oa_display::interaction::TargetView {
        address: first_identity.aircraft_address,
        generation: 8,
        identity_claims: 1,
        identity_present: 1,
        reserved: 0,
        identity: first_identity,
    };
    let replacement = oa_display::interaction::TargetView {
        generation: 9,
        identity: replacement_identity,
        ..first
    };
    let mut state = oa_display::interaction::interaction_new();
    assert_eq!(
        oa_display::interaction::select_target(
            &mut state,
            &[first],
            11,
            first.address,
            first.generation,
            Some(first_identity)
        ),
        oa_display::interaction::InteractionResult::Applied
    );
    assert_eq!(
        oa_display::interaction::enqueue_intent(
            &mut state,
            &[replacement],
            12,
            1,
            1,
            Intent::SetAssignedAltitude(15_000)
        ),
        oa_display::interaction::InteractionResult::Refused(
            oa_display::interaction::Refusal::StaleGeneration
        )
    );
}

// REQ: OA-REG-016
#[test]
fn fixture_console_journals_fresh_authorized_and_stale_requests() {
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/synthetic-session.tsv");
    let script = temp_path("script");
    let output = temp_path("out");
    let executable = &console_binaries()[0];
    fs::write(
        &script,
        b"NEXT\nNEXT\nSELECT ABC101\nACTOR 1\nASSIGNED 15000\nNEXT\nASSIGNED 16000\nQUIT\n",
    )
    .unwrap();
    let result = Command::new(executable)
        .args([
            "--input",
            input.to_str().unwrap(),
            "--synthetic-model",
            "--out",
            output.to_str().unwrap(),
            "--script",
            script.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let journal = fs::read_to_string(output.join("operator.tsv")).unwrap();
    assert!(journal.contains("SELECT\t"));
    assert!(journal.contains("QUEUE\t"));
    assert!(journal.contains("stale") || journal.contains("SELECT one bound current flight"));
    assert!(fs::read_to_string(output.join("frame-0003.txt"))
        .unwrap()
        .contains("Accepted"));
    assert!(fs::read_to_string(output.join("frame-0003.txt"))
        .unwrap()
        .contains("COMMANDS ACCEPTED 2 REFUSED 0; SELECT AGAIN"));
    verify_closed_bundle(&output);
    fs::remove_dir_all(&output).unwrap();
    fs::remove_file(script).unwrap();
    fs::remove_file(executable).unwrap();
}

// REQ: OA-REG-016
#[test]
fn full_recorded_batch_keeps_local_command_refusal_visible() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/synthetic-session.tsv"),
    )
    .unwrap();
    let input = temp_path("full-input");
    let script = temp_path("full-script");
    let output = temp_path("full-out");
    let executable = &console_binaries()[1];
    let mut recording = source
        .lines()
        .take_while(|line| !line.starts_with("v1\t3000000000\t"))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    for sequence in 1..=MAX_EVENTS_PER_RECEIPT {
        recording.push_str(&format!("v1\t3000000000\t3000000000\t{sequence}\tADAPTATION\t00000000000000000000000000000001\tABC101\tZAU1\tFEED\tUP\t0\n"));
    }
    fs::write(&input, recording).unwrap();
    fs::write(
        &script,
        b"NEXT\nNEXT\nSELECT ABC101\nACTOR 1\nASSIGNED 15000\nNEXT\nQUIT\n",
    )
    .unwrap();
    let result = Command::new(executable)
        .args([
            "--input",
            input.to_str().unwrap(),
            "--synthetic-model",
            "--script",
            script.to_str().unwrap(),
            "--out",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let frame = fs::read_to_string(output.join("frame-0003.txt")).unwrap();
    assert!(
        frame.contains("COMMANDS ACCEPTED 0 REFUSED 1; SELECT AGAIN"),
        "{frame}"
    );
    assert!(fs::read_to_string(output.join("operator.tsv"))
        .unwrap()
        .contains("receipt batch full"));
    verify_closed_bundle(&output);
    fs::remove_dir_all(output).unwrap();
    for path in [input, script] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_file(executable).unwrap();
}

// REQ: OA-RWY-025, OA-AUDIT-002, OA-AUDIT-003
#[test]
fn typed_runway_assignment_uses_normal_selection_queue_advance_and_pinned_audit() {
    use oa_regional_session::cli::ModelSource;

    let root = temp_path("typed-runway");
    fs::create_dir(&root).unwrap();
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/synthetic-session.tsv");
    let script = root.join("actions.txt");
    let output = root.join("output");
    fs::write(&script, b"NEXT\nNEXT\nSELECT ABC101\nACTOR 1\nNEXT\nQUIT\n").unwrap();
    let registry = root.join("principals.tsv");
    let registry_bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n";
    fs::write(&registry, registry_bytes).unwrap();
    let audit_log = root.join("actions.audit");
    let wrong_pin = run_script_session_with_typed_operator_inputs(
        &input,
        &root.join("wrong-pin-output"),
        &ModelSource::Synthetic,
        &script,
        &[TypedOperatorInput {
            after_action: 4,
            intent: Intent::AssignRunway(1),
        }],
        OperatorAuditConfiguration {
            registry_path: registry.clone(),
            registry_sha256: [0x55; 32],
            audit_log: audit_log.clone(),
            checkpoint: oa_audit_core::genesis(),
            principal_id: 51,
        },
    );
    assert!(
        wrong_pin.is_err(),
        "normal host entry refuses mismatched authority pin"
    );
    assert!(
        !audit_log.exists(),
        "wrong authority pin writes no action record"
    );
    let result = run_script_session_with_typed_operator_inputs(
        &input,
        &output,
        &ModelSource::Synthetic,
        &script,
        &[TypedOperatorInput {
            after_action: 4,
            intent: Intent::AssignRunway(1),
        }],
        OperatorAuditConfiguration {
            registry_path: registry,
            registry_sha256: oa_sha256::sha256(registry_bytes),
            audit_log: audit_log.clone(),
            checkpoint: oa_audit_core::genesis(),
            principal_id: 51,
        },
    );
    assert!(
        result.is_ok(),
        "typed queue path completes with an explicit refusal: {result:?}"
    );
    let journal = fs::read_to_string(output.join("operator.tsv")).unwrap();
    assert!(
        journal.contains("RUNWAY_COMMAND\t")
            && journal
                .contains("\tREFUSED\ttyped runway assignment does not name one issued runway end"),
        "{journal}"
    );
    assert!(
        !audit_log.exists(),
        "precondition refusal does not append audit"
    );
    verify_closed_bundle(&output);
    let _ = fs::remove_dir_all(root);
}

// REQ: OA-RWY-025, OA-AUDIT-002, OA-AUDIT-003
#[test]
fn typed_runway_host_queue_applies_issued_assignment_and_ack_on_normal_advances() {
    use oa_regional_session::cli::ModelSource;

    let root = temp_path("typed-runway-positive");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let (manifest_pin, package_root) = issue_synthetic_runway_package(&root);
    let input = root.join("session.tsv");
    fs::write(&input, synthetic_runway_session()).unwrap();
    let script = root.join("actions.txt");
    fs::write(
        &script,
        b"NEXT\nNEXT\nSELECT ABC101\nACTOR 1\nNEXT\nNEXT\nNEXT\nNEXT\nNEXT\nNEXT\nSELECT ABC101\nACTOR 1\nNEXT\nQUIT\n",
    )
    .unwrap();
    let registry_path = root.join("principals.tsv");
    let registry_bytes = b"OACERT-LOCAL-PRINCIPALS-V1\n51\toperate\n";
    fs::write(&registry_path, registry_bytes).unwrap();
    let audit_log = root.join("actions.audit");
    let output = root.join("output");
    let result = run_script_session_with_typed_operator_inputs(
        &input,
        &output,
        &ModelSource::IssuedPackage {
            root: package_root,
            expected_manifest_sha256: manifest_pin,
            processor_id: 51,
        },
        &script,
        &[
            TypedOperatorInput {
                after_action: 4,
                intent: Intent::AssignRunway(1),
            },
            TypedOperatorInput {
                after_action: 12,
                intent: Intent::AcknowledgeRunwayAlert,
            },
        ],
        OperatorAuditConfiguration {
            registry_path,
            registry_sha256: oa_sha256::sha256(registry_bytes),
            audit_log: audit_log.clone(),
            checkpoint: oa_audit_core::genesis(),
            principal_id: 51,
        },
    );
    assert!(
        result.is_ok(),
        "normal typed host path succeeds: {result:?}"
    );

    let journal = fs::read_to_string(output.join("operator.tsv")).unwrap();
    let applied: Vec<_> = journal
        .lines()
        .filter(|line| line.starts_with("RUNWAY_COMMAND\t") && line.contains("\tAPPLIED\t"))
        .collect();
    assert_eq!(applied.len(), 2, "both queued actions apply: {journal}");
    assert!(journal.contains("\tAPPLIED\tcheckpoint=1"), "{journal}");
    assert!(journal.contains("\tAPPLIED\tcheckpoint=2"), "{journal}");

    let alert_frame = fs::read_to_string(output.join("frame-0008.txt")).unwrap();
    assert!(alert_frame.contains("status: WrongRunway"), "{alert_frame}");
    assert!(alert_frame.contains("acknowledged: 0"), "{alert_frame}");
    let ack_frame = fs::read_to_string(output.join("frame-0009.txt")).unwrap();
    assert!(ack_frame.contains("status: WrongRunway"), "{ack_frame}");
    assert!(ack_frame.contains("acknowledged: 1"), "{ack_frame}");
    assert!(
        audit_log.is_file(),
        "both applied actions have an audit log"
    );
    verify_closed_bundle(&output);
    let _ = fs::remove_dir_all(root);
}

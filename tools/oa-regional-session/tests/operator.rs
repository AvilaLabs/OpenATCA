#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use oa_display::interaction::{FlightIdentity, Intent};
use oa_regional_session::input::{
    EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind,
};
use oa_regional_session::operator::{
    operator_journal_bytes, operator_journal_new, parse_action, parse_actions, prepare_batch,
    Action, RecordedBatch, RecordedEvent,
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
                    source_message_id: None,
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
                    source_message_id: None,
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

// Mutation killed: removing the one-time full-key identity or matching observation callsign
// prevents selection of the owned synthetic flight, so the command journal loses acceptance.
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

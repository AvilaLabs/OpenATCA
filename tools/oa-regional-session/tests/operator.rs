use oa_regional_session::input::{EventKind, FlightOperation, Identity, NormalizedEvent, SourceKind};
use oa_regional_session::operator::{
    parse_action, parse_actions, prepare_batch, Action, OperatorJournal, RecordedBatch, RecordedEvent,
};
use oa_regional_session::session::MAX_EVENTS_PER_RECEIPT;
use oa_regional_session::cli::ModelSource;
use oa_display::interaction::{FlightIdentity, Intent};
use oa_regional_session::operator::run_script_session;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn temp_path(label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!("oa-regional-operator-{label}-{}-{}",
        std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

#[test]
fn grammar_is_explicit_and_bounded() {
    assert_eq!(parse_action("SELECT ABC101"), Ok(Action::Select(0x00ab_c101)));
    assert_eq!(parse_action("ACTOR 2"), Ok(Action::Actor(2)));
    assert!(parse_action("SELECT 0").is_err());
    assert!(parse_action("ASSIGNED 12000.5").is_err());
    assert!(parse_action("NEXT trailing").is_err());
    assert!(parse_action(&"N".repeat(129)).is_err());
    assert!(parse_actions(b"NEXT\nQUIT\nNEXT\n").is_err());
}

#[test]
fn local_events_are_inserted_before_route_and_sequences_are_audited() {
    let identity_input = Identity { flight_gufi: 1, aircraft_address: 0x00ab_c101 };
    let recorded = RecordedBatch { receipt_ns: 1_000_000_000, source_clock_regressions: 0, events: vec![
        RecordedEvent { event: NormalizedEvent { receipt_ns: 1_000_000_000, source_ns: 1_000_000_000,
            sequence: 9, source: SourceKind::Flight, identity: identity_input, centre: *b"ZAU1",
            kind: EventKind::Flight(FlightOperation::ClearAssignedAltitude) }, original_sequence: 9, ordinal: 1 },
        RecordedEvent { event: NormalizedEvent { receipt_ns: 1_000_000_000, source_ns: 1_000_000_000,
            sequence: 7, source: SourceKind::Route, identity: identity_input, centre: *b"ZAU1",
            kind: EventKind::FeedState { available: true, reason: 0 } }, original_sequence: 7, ordinal: 2 },
    ] };
    let identity = FlightIdentity { gufi: [1; 16], aircraft_address: 0x00ab_c101,
        centre: *b"ZAU1", plan_id: *b"PLAN00000001" };
    let selected = oa_regional_session::operator::SelectedTarget {
        view: oa_display::interaction::TargetView { address: identity.aircraft_address, generation: 3,
            identity_claims: 1, identity_present: 1, reserved: 0, identity },
        identity, view_sequence: 4,
    };
    let command = oa_regional_session::operator::make_command(selected, 1, Intent::SetAssignedAltitude(15_000), 1).unwrap();
    let mut sequences = [0; 5];
    let mut journal = OperatorJournal::new();
    let prepared = prepare_batch(&recorded, &[command], &mut sequences, &mut journal).unwrap();
    assert_eq!(prepared.events.len(), 3);
    assert_eq!(prepared.events[0].source, SourceKind::Flight);
    assert!(matches!(prepared.events[1].kind, EventKind::Operator(_)));
    assert_eq!(prepared.events[2].source, SourceKind::Route);
    assert_eq!(prepared.events[0].sequence, 1);
    assert_eq!(prepared.events[1].sequence, 2);
    assert_eq!(prepared.events[2].sequence, 1);
    assert!(std::str::from_utf8(journal.bytes()).unwrap().contains("RESEQUENCE\t"));
    assert!(std::str::from_utf8(journal.bytes()).unwrap().contains("LOCAL\t1\tFLIGHT"));
    assert!(prepare_batch(&recorded, &vec![command; MAX_EVENTS_PER_RECEIPT], &mut sequences, &mut journal).is_err());
}

#[test]
fn explicit_selection_refuses_ambiguous_or_unbound_address() {
    let identity = FlightIdentity { gufi: [1; 16], aircraft_address: 0x00ab_c101,
        centre: *b"ZAU1", plan_id: *b"PLAN00000001" };
    let view = oa_display::interaction::TargetView { address: identity.aircraft_address, generation: 2,
        identity_claims: 1, identity_present: 1, reserved: 0, identity };
    assert!(oa_regional_session::operator::selected_target(&[view], view.address, 3).is_ok());
    let duplicate = oa_display::interaction::TargetView { generation: 3, ..view };
    assert!(oa_regional_session::operator::selected_target(&[view, duplicate], view.address, 3).is_err());
    let raw = oa_display::interaction::TargetView { identity_present: 0, ..view };
    assert!(oa_regional_session::operator::selected_target(&[raw], view.address, 3).is_err());
}

#[test]
fn fixture_console_journals_fresh_authorized_and_stale_requests() {
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/synthetic-session.tsv");
    let script = temp_path("script");
    let output = temp_path("out");
    fs::write(&script, b"NEXT\nSELECT ABC101\nACTOR 1\nASSIGNED 15000\nNEXT\nASSIGNED 16000\nQUIT\n").unwrap();
    run_script_session(&input, &output, &ModelSource::Synthetic, &script).unwrap();
    let journal = fs::read_to_string(output.join("operator.tsv")).unwrap();
    assert!(journal.contains("SELECT\t"));
    assert!(journal.contains("QUEUE\t"));
    assert!(journal.contains("stale") || journal.contains("SELECT one bound current flight"));
    assert!(output.join("COMPLETE").is_file());
    assert!(fs::read_to_string(output.join("manifest.tsv")).unwrap().contains("settings_sha256\t"));
    fs::remove_dir_all(&output).unwrap();
    fs::remove_file(script).unwrap();
}

use std::path::Path;

// REQ: OA-TRC-001
#[test]
fn actual_workspace_has_no_requirement_trace_errors() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let report = oa_trace::analyze(&workspace);
    let rendered = oa_trace::render_text(&report);

    assert!(
        report.io_errors.is_empty(),
        "workspace scan errors:\n{rendered}"
    );
    assert!(
        report.untested.is_empty(),
        "requirements without tests or proofs:\n{rendered}"
    );
    assert!(
        report.unknown.is_empty(),
        "tags naming unknown requirements:\n{rendered}"
    );
    assert!(
        report.malformed.is_empty(),
        "malformed requirement tags:\n{rendered}"
    );
    assert!(
        report.duplicates.is_empty(),
        "duplicate requirement definitions:\n{rendered}"
    );
    assert!(report.ok(), "workspace traceability must pass:\n{rendered}");
}

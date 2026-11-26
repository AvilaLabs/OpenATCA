use std::path::Path;

// REQ: OA-MIG-602
#[test]
fn repository_hazard_links_resolve_to_active_requirements_and_verification() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let errors = oa_trace::hazard_trace::check(&root);
    assert!(errors.is_empty(), "hazard trace errors: {errors:#?}");
}

use super::*;

fn event(action: Action, role: Role) -> Event {
    let (target, target_len) = target(b"unit-1").expect("test target");
    Event {
        principal: 7,
        role,
        action,
        target,
        target_len,
        time_ns: 12,
        details: [0; DETAILS_CAPACITY],
        details_len: 0,
    }
}

// REQ: OA-AUDIT-001, OA-AUDIT-002
#[test]
fn bounded_event_registry_and_exact_receipt_binding() {
    let flip = event(Action::MigrationFlip, Role::Configure);
    assert_eq!(validate_event(&flip), Ok(()));
    assert_eq!(
        authorize(Role::Operate, Action::MigrationRollback),
        Err(AuditError::RoleDenied)
    );
    assert_eq!(authorize(Role::Operate, Action::QuarantineRelease), Ok(()));
    let receipt = receipt_claim(1, [9; 32], [0; 32], flip);
    assert!(receipt_matches(&receipt, flip));
    assert!(!receipt_matches(
        &receipt,
        event(Action::MigrationRollback, Role::Configure)
    ));
}

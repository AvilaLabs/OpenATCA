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

// REQ: OA-AUDIT-001, OA-AUDIT-002, OA-AUDIT-005
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

// REQ: OA-AUDIT-002, OA-AUDIT-003, OA-AUDIT-005, OA-AUDIT-010
#[test]
fn msaw_service_and_hold_command_actions_have_distinct_receipt_bindings() {
    let msaw = event(Action::MsawServiceRequest, Role::Operate);
    let hold = event(Action::HoldCommand, Role::Operate);
    assert_eq!(msaw.action as u8, 13);
    assert_eq!(hold.action as u8, 14);
    assert_eq!(validate_event(&msaw), Ok(()));
    assert_eq!(validate_event(&hold), Ok(()));

    let msaw_receipt = receipt_claim(1, [1; 32], [0; 32], msaw);
    let hold_receipt = receipt_claim(1, [2; 32], [0; 32], hold);
    assert!(receipt_matches(&msaw_receipt, msaw));
    assert!(!receipt_matches(&msaw_receipt, hold));
    assert!(receipt_matches(&hold_receipt, hold));
    assert!(!receipt_matches(&hold_receipt, msaw));
}

// REQ: OA-AUDIT-002, OA-AUDIT-010
#[test]
fn registry_action_matrix_keeps_added_actions_at_the_declared_levels() {
    for action in [
        Action::QuarantineRelease,
        Action::VulnerabilityIntake,
        Action::RunwayAssignment,
        Action::AlertAcknowledge,
        Action::MsawServiceRequest,
        Action::HoldCommand,
    ] {
        assert_eq!(
            authorize(Role::Monitor, action),
            Err(AuditError::RoleDenied)
        );
        assert_eq!(authorize(Role::Operate, action), Ok(()));
        assert_eq!(authorize(Role::Configure, action), Ok(()));
    }
    assert_eq!(
        authorize(Role::Monitor, Action::VulnerabilityDisposition),
        Err(AuditError::RoleDenied)
    );
    assert_eq!(
        authorize(Role::Operate, Action::VulnerabilityDisposition),
        Err(AuditError::RoleDenied)
    );
    assert_eq!(
        authorize(Role::Configure, Action::VulnerabilityDisposition),
        Ok(())
    );
}

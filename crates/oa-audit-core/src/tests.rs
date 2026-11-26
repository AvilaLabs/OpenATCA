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

// REQ: OA-AUDIT-002, OA-AUDIT-009
#[test]
fn vulnerability_and_runway_actions_enforce_registry_role_levels() {
    assert_eq!(
        authorize(Role::Monitor, Action::VulnerabilityIntake),
        Err(AuditError::RoleDenied)
    );
    assert_eq!(
        authorize(Role::Operate, Action::VulnerabilityIntake),
        Ok(())
    );
    assert_eq!(
        authorize(Role::Configure, Action::VulnerabilityIntake),
        Ok(())
    );
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
    assert_eq!(
        authorize(Role::Monitor, Action::RunwayAssignment),
        Err(AuditError::RoleDenied)
    );
    assert_eq!(authorize(Role::Operate, Action::RunwayAssignment), Ok(()));
    assert_eq!(authorize(Role::Configure, Action::RunwayAssignment), Ok(()));
}

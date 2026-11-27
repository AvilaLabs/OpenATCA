use e04_mutants_fixture::safety_decision;

#[test]
fn decision_requires_active_matching_pair() {
    assert!(!safety_decision(false, false));
    assert!(!safety_decision(false, true));
    assert!(!safety_decision(true, false));
    assert!(safety_decision(true, true));
}

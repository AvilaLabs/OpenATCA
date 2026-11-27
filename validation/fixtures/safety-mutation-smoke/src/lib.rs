pub fn safety_decision(active: bool, matching_key: bool) -> bool {
    active && matching_key
}

pub fn unobserved_diagnostic(enabled: bool) -> bool {
    enabled
}

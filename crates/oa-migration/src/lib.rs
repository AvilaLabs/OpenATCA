#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-migration is a bounded, pure lifecycle evaluator for a single migration swap unit.
//! A unit is one function, one scope and one explicit window. It does not switch outputs,
//! connect to live systems, authenticate people, or inspect evidence artifacts. Evidence and
//! authority handles bind external records; this crate checks their declared scope, freshness,
//! policy binding, numeric measurements and required role set. Time is always supplied by the
//! caller (RULESET B1-B4, H1).

// The core has a single supported word width (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-migration core supports 64-bit targets only");

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    S0Shadow = 0,
    S1Advisory = 1,
    S2Window = 2,
    S3Extended = 3,
    S4Continuous = 4,
    S5Retired = 5,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapUnitKey {
    pub function_id: u32,
    pub scope_id: u32,
    pub window_start_ns: i64,
    pub window_end_ns: i64,
}
const _: () = assert!(core::mem::size_of::<SwapUnitKey>() == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapUnit {
    key: SwapUnitKey,
    stage: Stage,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    last_transition_ns: i64,
    audit_sequence: u32,
    has_prior_dependency: bool,
    prior_dependency: DependencyEvidence,
}
// Bounded state for one unit; arrays of units are owned by the caller.
const _: () = assert!(core::mem::size_of::<SwapUnit>() == 256);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    AtMost = 0,
    AtLeast = 1,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceQuality {
    Complete = 0,
    Incomplete = 1,
    Unresolved = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricCriterion {
    pub metric_id: u16,
    pub comparison: Comparison,
    pub threshold: i64,
    pub minimum_samples: u32,
    pub minimum_denominator: u32,
    pub minimum_exposure_ns: u64,
}
// A7.2: cold, predeclared transition policy record.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionPolicy {
    pub key: SwapUnitKey,
    pub from: Stage,
    pub to: Stage,
    pub policy_version: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub maximum_evidence_age_ns: u64,
    pub required_role_mask: u8,
    pub criterion_count: u8,
    pub criteria: [MetricCriterion; MAX_CRITERIA],
    pub dependency_count: u8,
    pub dependencies: [DependencyRequirement; MAX_DEPENDENCIES],
    pub rollback_policy: RollbackTriggerPolicy,
}
// A7.2: cold, one-shot caller-supplied policy; its digest must match the approved unit record.

pub const MAX_CRITERIA: usize = 8;
pub const MAX_DEPENDENCIES: usize = 8;
pub const MAX_EVIDENCE: usize = 8;
pub const MAX_APPROVALS: usize = 8;
pub const MAX_AUDIT_EVENTS: usize = 32;
pub const S3_MIN_EXPOSURE_NS: u64 = 172_800_000_000_000;
pub const S4_MIN_EXPOSURE_NS: u64 = 604_800_000_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyRequirement {
    pub key: SwapUnitKey,
    pub minimum_stage: Stage,
    pub policy_version: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
}
// A7.2: cold, bounded dependency declaration.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RollbackRule {
    pub enabled: bool,
    pub maximum_response_ns: u64,
    pub required_role_mask: u8,
}
// A7.2: cold, predeclared rollback response target and authority requirement.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RollbackTriggerPolicy {
    pub blocking_difference: RollbackRule,
    pub controller_request: RollbackRule,
    pub health_fault: RollbackRule,
}
// A7.2: cold typed policy for the three documented rollback causes.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricEvidence {
    pub key: SwapUnitKey,
    pub measured_stage: Stage,
    pub metric_id: u16,
    pub quality: EvidenceQuality,
    pub policy_version: u32,
    pub samples: u32,
    pub denominator: u32,
    pub observed: i64,
    pub exposure_ns: u64,
    pub observation_start_ns: i64,
    pub observation_end_ns: i64,
    pub observed_at_ns: i64,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub artifact_handle: [u8; 32],
    pub evidence_set_handle: [u8; 32],
    pub observation_origin_key: SwapUnitKey,
    pub origin_stage_activation_ns: i64,
    pub origin_receipt_handle: [u8; 32],
}
// A7.2: cold, bounded one-transition metric evidence record.
const _: () = assert!(core::mem::size_of::<MetricEvidence>() == 280);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityRole {
    FacilitySupervisor = 0,
    ControllerRepresentative = 1,
    TechnicalOperations = 2,
    IndependentAssessor = 3,
    FacilityRetirementAuthority = 4,
    ControllerOnPosition = 5,
}

pub const ROLE_SUPERVISOR: u8 = 1 << AuthorityRole::FacilitySupervisor as u8;
pub const ROLE_CONTROLLER: u8 = 1 << AuthorityRole::ControllerRepresentative as u8;
pub const ROLE_TECH_OPS: u8 = 1 << AuthorityRole::TechnicalOperations as u8;
pub const ROLE_ASSESSOR: u8 = 1 << AuthorityRole::IndependentAssessor as u8;
pub const ROLE_RETIREMENT: u8 = 1 << AuthorityRole::FacilityRetirementAuthority as u8;
pub const ROLE_CONTROLLER_ON_POSITION: u8 = 1 << AuthorityRole::ControllerOnPosition as u8;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorityApproval {
    pub key: SwapUnitKey,
    pub from: Stage,
    pub to: Stage,
    pub role: AuthorityRole,
    pub approved: bool,
    pub principal_id: u32,
    pub approved_at_ns: i64,
    pub policy_version: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub evidence_set_handle: [u8; 32],
    pub approval_set_handle: [u8; 32],
    pub authority_record_handle: [u8; 32],
}
// A7.2: cold, one-shot references to externally retained authority records.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyEvidence {
    key: SwapUnitKey,
    achieved_stage: Stage,
    quality: EvidenceQuality,
    observed_at_ns: i64,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    artifact_handle: [u8; 32],
}
// A7.2: cold dependency receipt emitted from an opaque successful prior transition.
const _: () = assert!(core::mem::size_of::<DependencyEvidence>() == 144);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    None = 0,
    InvalidUnit = 1,
    InvalidPolicy = 2,
    WrongStage = 3,
    StaleEvidence = 4,
    EvidenceScopeMismatch = 5,
    EvidenceMissing = 6,
    EvidenceLimit = 7,
    MetricFailed = 8,
    DependencyMissing = 9,
    DependencyMismatch = 10,
    AuthorityMissing = 11,
    AuthorityRejected = 12,
    AuthorityLimit = 13,
    RetirementAuthorityRequired = 14,
    TimeInvalid = 15,
    AuditFull = 16,
    CounterOverflow = 17,
    DependencyLimit = 18,
    EvidenceIncomplete = 19,
    EvidenceUnresolved = 20,
    AuthorityConflict = 21,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    pub key: SwapUnitKey,
    pub sequence: u32,
    pub evaluated_at_ns: i64,
    pub from: Stage,
    pub to: Stage,
    pub refusal: Refusal,
    pub policy_version: u32,
    pub principal_id: u32,
    pub assessor_principal_id: u32,
    pub policy_digest: [u8; 32],
    pub source_epoch_digest: [u8; 32],
    pub evidence_set_handle: [u8; 32],
    pub approval_set_handle: [u8; 32],
}
// Bounded, append-only-by-caller audit record; caller must not overwrite a prior event.
const _: () = assert!(core::mem::size_of::<AuditEvent>() == 184);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditLog {
    entries: [AuditEvent; MAX_AUDIT_EVENTS],
    len: u8,
}
// A7.2: cold, bounded, append-only decision history for one caller-owned scope of work.
const _: () = assert!(core::mem::size_of::<AuditLog>() == 5896);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionResult {
    unit: SwapUnit,
    event: AuditEvent,
}
// A7.2: cold result returned to caller for explicit append to its bounded audit array.

#[derive(Clone, Copy)]
pub struct TransitionInput<'a> {
    pub policy: &'a TransitionPolicy,
    pub metrics: &'a [MetricEvidence],
    pub dependencies: &'a [DependencyEvidence],
    pub approvals: &'a [AuthorityApproval],
    pub evidence_set_handle: [u8; 32],
    pub approval_set_handle: [u8; 32],
    pub now_ns: i64,
}
// A7.2: cold one-call view over caller-owned bounded transition inputs.

#[derive(Clone, Copy)]
pub struct ContinuationStart {
    unit: SwapUnit,
    dependency: DependencyEvidence,
}
// A7.2: cold, opaque result of validating the prior successful rung for a new window.

pub const fn empty_audit_log() -> AuditLog {
    let blank = AuditEvent {
        key: SwapUnitKey {
            function_id: 0,
            scope_id: 0,
            window_start_ns: 0,
            window_end_ns: 0,
        },
        sequence: 0,
        evaluated_at_ns: 0,
        from: Stage::S0Shadow,
        to: Stage::S0Shadow,
        refusal: Refusal::None,
        policy_version: 0,
        principal_id: 0,
        assessor_principal_id: 0,
        policy_digest: [0; 32],
        source_epoch_digest: [0; 32],
        evidence_set_handle: [0; 32],
        approval_set_handle: [0; 32],
    };
    AuditLog {
        entries: [blank; MAX_AUDIT_EVENTS],
        len: 0,
    }
}

pub const fn swap_unit_key(unit: &SwapUnit) -> SwapUnitKey {
    unit.key
}

pub const fn swap_unit_stage(unit: &SwapUnit) -> Stage {
    unit.stage
}

pub const fn swap_unit_audit_sequence(unit: &SwapUnit) -> u32 {
    unit.audit_sequence
}

pub const fn swap_unit_policy_version(unit: &SwapUnit) -> u32 {
    unit.policy_version
}

pub const fn swap_unit_policy_digest(unit: &SwapUnit) -> [u8; 32] {
    unit.policy_digest
}

pub const fn swap_unit_source_epoch_digest(unit: &SwapUnit) -> [u8; 32] {
    unit.source_epoch_digest
}

pub const fn audit_len(log: &AuditLog) -> u8 {
    log.len
}

pub fn audit_event(log: &AuditLog, index: usize) -> Option<AuditEvent> {
    if index >= usize::from(log.len) {
        return None;
    }
    log.entries.get(index).copied()
}

/// Creates a new unit in S0; no implicit authority, evidence, or transition is inferred.
pub fn new_unit(
    key: SwapUnitKey,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    created_at_ns: i64,
) -> Result<SwapUnit, Refusal> {
    if key.function_id == 0
        || key.scope_id == 0
        || key.window_start_ns >= key.window_end_ns
        || created_at_ns >= key.window_end_ns
    {
        return Err(Refusal::InvalidUnit);
    }
    if policy_version == 0
        || !handle_present(&policy_digest)
        || !handle_present(&source_epoch_digest)
    {
        return Err(Refusal::InvalidPolicy);
    }
    Ok(SwapUnit {
        key,
        stage: Stage::S0Shadow,
        policy_version,
        policy_digest,
        source_epoch_digest,
        last_transition_ns: created_at_ns,
        audit_sequence: 0,
        has_prior_dependency: false,
        prior_dependency: blank_dependency(),
    })
}

/// Starts a later exposure window at the last rung reached by an opaque successful result.
/// The new window must be adjacent or later, and a corresponding dependency receipt is returned
/// and embedded in the new unit so the next transition policy must require the exact prior unit.
pub fn new_unit_continuation(
    prior: &TransitionResult,
    key: SwapUnitKey,
    policy_version: u32,
    policy_digest: [u8; 32],
    source_epoch_digest: [u8; 32],
    created_at_ns: i64,
    prior_receipt_handle: [u8; 32],
) -> Result<ContinuationStart, Refusal> {
    let prior_unit = &prior.unit;
    let prior_event = &prior.event;
    if prior_event.refusal != Refusal::None
        || prior_event.key != prior_unit.key
        || prior_event.to != prior_unit.stage
        || prior_event.sequence != prior_unit.audit_sequence
        || prior_event.policy_version != prior_unit.policy_version
        || prior_event.policy_digest != prior_unit.policy_digest
        || prior_event.source_epoch_digest != prior_unit.source_epoch_digest
        || prior_event.principal_id == 0
        || prior_event.assessor_principal_id == 0
        || prior_event.principal_id == prior_event.assessor_principal_id
        || !handle_present(&prior_event.evidence_set_handle)
        || !handle_present(&prior_event.approval_set_handle)
        || prior_unit.stage == Stage::S0Shadow
        || prior_unit.stage == Stage::S5Retired
    {
        return Err(Refusal::DependencyMismatch);
    }
    if !valid_key(key)
        || key.function_id != prior_unit.key.function_id
        || key.scope_id != prior_unit.key.scope_id
        || key.window_start_ns < prior_unit.key.window_end_ns
        || created_at_ns < prior_event.evaluated_at_ns
        || created_at_ns >= key.window_end_ns
    {
        return Err(Refusal::InvalidUnit);
    }
    if policy_version == 0
        || !handle_present(&policy_digest)
        || !handle_present(&source_epoch_digest)
        || !handle_present(&prior_receipt_handle)
    {
        return Err(Refusal::InvalidPolicy);
    }
    let unit = SwapUnit {
        key,
        stage: prior_unit.stage,
        policy_version,
        policy_digest,
        source_epoch_digest,
        last_transition_ns: created_at_ns,
        audit_sequence: 0,
        has_prior_dependency: false,
        prior_dependency: blank_dependency(),
    };
    let dependency = DependencyEvidence {
        key: prior_unit.key,
        achieved_stage: prior_unit.stage,
        quality: EvidenceQuality::Complete,
        observed_at_ns: prior_event.evaluated_at_ns,
        policy_version: prior_unit.policy_version,
        policy_digest: prior_unit.policy_digest,
        source_epoch_digest: prior_unit.source_epoch_digest,
        artifact_handle: prior_receipt_handle,
    };
    let mut unit = unit;
    unit.has_prior_dependency = true;
    unit.prior_dependency = dependency;
    Ok(ContinuationStart { unit, dependency })
}

const fn blank_dependency() -> DependencyEvidence {
    DependencyEvidence {
        key: SwapUnitKey {
            function_id: 0,
            scope_id: 0,
            window_start_ns: 0,
            window_end_ns: 0,
        },
        achieved_stage: Stage::S0Shadow,
        quality: EvidenceQuality::Unresolved,
        observed_at_ns: 0,
        policy_version: 0,
        policy_digest: [0; 32],
        source_epoch_digest: [0; 32],
        artifact_handle: [0; 32],
    }
}

pub const fn continuation_unit(start: &ContinuationStart) -> SwapUnit {
    start.unit
}

pub const fn continuation_dependency(start: &ContinuationStart) -> DependencyEvidence {
    start.dependency
}

pub const fn dependency_requirement(
    receipt: &DependencyEvidence,
    minimum_stage: Stage,
) -> DependencyRequirement {
    DependencyRequirement {
        key: receipt.key,
        minimum_stage,
        policy_version: receipt.policy_version,
        policy_digest: receipt.policy_digest,
        source_epoch_digest: receipt.source_epoch_digest,
    }
}

pub const fn transition_unit(result: &TransitionResult) -> SwapUnit {
    result.unit
}

pub const fn transition_event(result: &TransitionResult) -> AuditEvent {
    result.event
}

/// Evaluates exactly one adjacent rung. It validates numeric measurements and declared
/// authority roles but cannot verify artifact contents or authenticate principals.
/// The returned event, including refusals, is appended to the explicit bounded log. Persist the
/// returned unit even on refusal to preserve its monotonic audit sequence; its stage remains put.
pub fn evaluate_transition(
    unit: &SwapUnit,
    input: TransitionInput<'_>,
    audit_log: &mut AuditLog,
) -> TransitionResult {
    let policy = input.policy;
    let approvals = input.approvals;
    let mut event = AuditEvent {
        key: unit.key,
        sequence: unit.audit_sequence,
        evaluated_at_ns: input.now_ns,
        from: unit.stage,
        to: policy.to,
        refusal: Refusal::None,
        policy_version: unit.policy_version,
        principal_id: 0,
        assessor_principal_id: 0,
        policy_digest: unit.policy_digest,
        source_epoch_digest: unit.source_epoch_digest,
        evidence_set_handle: input.evidence_set_handle,
        approval_set_handle: input.approval_set_handle,
    };
    let mut next = *unit;
    let remaining = if usize::from(audit_log.len) <= MAX_AUDIT_EVENTS {
        MAX_AUDIT_EVENTS - usize::from(audit_log.len)
    } else {
        0
    };
    let refusal = validate_transition(unit, &input, remaining);
    event.refusal = refusal;
    if refusal != Refusal::AuditFull {
        match unit.audit_sequence.checked_add(1) {
            Some(sequence) => {
                next.audit_sequence = sequence;
                event.sequence = sequence;
                if refusal == Refusal::None {
                    next.stage = policy.to;
                    next.last_transition_ns = input.now_ns;
                    next.has_prior_dependency = false;
                    next.prior_dependency = blank_dependency();
                    event.principal_id = approving_principal(
                        approvals,
                        unit.key,
                        policy,
                        input.evidence_set_handle,
                        input.approval_set_handle,
                    );
                    event.assessor_principal_id = approval_principal_for_role(
                        approvals,
                        unit.key,
                        policy,
                        input.evidence_set_handle,
                        input.approval_set_handle,
                        AuthorityRole::IndependentAssessor,
                    );
                }
                if let Some(slot) = audit_log.entries.get_mut(usize::from(audit_log.len)) {
                    *slot = event;
                    audit_log.len = match audit_log.len.checked_add(1) {
                        Some(value) => value,
                        None => {
                            event.refusal = Refusal::CounterOverflow;
                            next = *unit;
                            audit_log.len
                        }
                    };
                } else {
                    event.refusal = Refusal::AuditFull;
                    next = *unit;
                }
            }
            None => {
                event.refusal = Refusal::CounterOverflow;
            }
        }
    }
    TransitionResult { unit: next, event }
}

fn validate_transition(
    unit: &SwapUnit,
    input: &TransitionInput<'_>,
    audit_slots_remaining: usize,
) -> Refusal {
    let policy = input.policy;
    let evidence = input.metrics;
    let dependencies = input.dependencies;
    let approvals = input.approvals;
    let evidence_set_handle = input.evidence_set_handle;
    let approval_set_handle = input.approval_set_handle;
    let now_ns = input.now_ns;
    if audit_slots_remaining == 0 {
        return Refusal::AuditFull;
    }
    if audit_slots_remaining > MAX_AUDIT_EVENTS {
        return Refusal::InvalidPolicy;
    }
    if now_ns < unit.last_transition_ns {
        return Refusal::TimeInvalid;
    }
    if !valid_key(unit.key)
        || unit.policy_version == 0
        || !handle_present(&unit.policy_digest)
        || !handle_present(&unit.source_epoch_digest)
    {
        return Refusal::InvalidUnit;
    }
    if policy.key != unit.key
        || policy.policy_version != unit.policy_version
        || policy.policy_digest != unit.policy_digest
        || policy.source_epoch_digest != unit.source_epoch_digest
        || policy.maximum_evidence_age_ns == 0
        || !handle_present(&evidence_set_handle)
        || !handle_present(&approval_set_handle)
        || usize::from(policy.criterion_count) == 0
        || usize::from(policy.criterion_count) > MAX_CRITERIA
        || usize::from(policy.dependency_count) > MAX_DEPENDENCIES
    {
        return Refusal::InvalidPolicy;
    }
    if evidence.len() > MAX_EVIDENCE {
        return Refusal::EvidenceLimit;
    }
    if dependencies.len() > MAX_DEPENDENCIES {
        return Refusal::DependencyLimit;
    }
    if approvals.len() > MAX_APPROVALS {
        return Refusal::AuthorityLimit;
    }
    if policy.required_role_mask & !0x3f != 0 {
        return Refusal::InvalidPolicy;
    }
    if !valid_rollback_policy(policy.rollback_policy) {
        return Refusal::InvalidPolicy;
    }
    if policy.from != unit.stage || next_stage(unit.stage) != Some(policy.to) {
        return Refusal::WrongStage;
    }
    let unit_span = match unit.key.window_end_ns.checked_sub(unit.key.window_start_ns) {
        Some(span) if span > 0 => match u64::try_from(span) {
            Ok(value) => value,
            Err(_) => return Refusal::InvalidUnit,
        },
        _ => return Refusal::InvalidUnit,
    };
    let minimum_window = match policy.to {
        Stage::S3Extended => S3_MIN_EXPOSURE_NS,
        Stage::S4Continuous => S4_MIN_EXPOSURE_NS,
        _ => 0,
    };
    if minimum_window > 0 && unit_span < minimum_window {
        return Refusal::InvalidPolicy;
    }
    let minimum_prior_exposure = match policy.from {
        Stage::S3Extended => S3_MIN_EXPOSURE_NS,
        Stage::S4Continuous => S4_MIN_EXPOSURE_NS,
        _ => 0,
    };
    if minimum_prior_exposure > 0 && !has_minimum_exposure(policy, minimum_prior_exposure) {
        return Refusal::InvalidPolicy;
    }
    if unit.stage == Stage::S4Continuous
        && policy.to == Stage::S5Retired
        && policy.required_role_mask & ROLE_RETIREMENT == 0
    {
        return Refusal::RetirementAuthorityRequired;
    }
    match policy.to {
        Stage::S1Advisory if now_ns >= unit.key.window_end_ns => return Refusal::TimeInvalid,
        Stage::S2Window | Stage::S3Extended | Stage::S4Continuous | Stage::S5Retired
            if now_ns < unit.key.window_start_ns || now_ns >= unit.key.window_end_ns =>
        {
            return Refusal::TimeInvalid;
        }
        _ => {}
    }
    let minimum_roles = mandatory_roles(policy.to);
    if policy.required_role_mask & minimum_roles != minimum_roles {
        return Refusal::InvalidPolicy;
    }
    for index in 0..usize::from(policy.criterion_count) {
        match policy.criteria.get(index) {
            Some(criterion)
                if criterion.metric_id != 0
                    && criterion.minimum_samples > 0
                    && criterion.minimum_denominator > 0 => {}
            _ => return Refusal::InvalidPolicy,
        }
    }
    for index in 0..usize::from(policy.dependency_count) {
        match policy.dependencies.get(index) {
            Some(dep)
                if valid_key(dep.key)
                    && dep.key != unit.key
                    && dep.policy_version > 0
                    && handle_present(&dep.policy_digest)
                    && handle_present(&dep.source_epoch_digest) => {}
            _ => return Refusal::InvalidPolicy,
        }
        for prior_index in 0..index {
            if let Some(prior) = policy.dependencies.get(prior_index) {
                if let Some(current) = policy.dependencies.get(index) {
                    if prior.key == current.key {
                        return Refusal::InvalidPolicy;
                    }
                }
            }
        }
    }
    if unit.has_prior_dependency {
        let prior = unit.prior_dependency;
        let mut required_receipt = false;
        for index in 0..usize::from(policy.dependency_count) {
            if let Some(requirement) = policy.dependencies.get(index) {
                if requirement.key == prior.key
                    && requirement.minimum_stage == prior.achieved_stage
                    && requirement.policy_version == prior.policy_version
                    && requirement.policy_digest == prior.policy_digest
                    && requirement.source_epoch_digest == prior.source_epoch_digest
                {
                    required_receipt = true;
                }
            }
        }
        if !required_receipt {
            return Refusal::DependencyMissing;
        }
    }
    let dependency_result = dependencies_valid(unit, policy, dependencies, now_ns);
    if dependency_result != Refusal::None {
        return dependency_result;
    }
    let evidence_result = metrics_valid(unit, policy, evidence, evidence_set_handle, now_ns);
    if evidence_result != Refusal::None {
        return evidence_result;
    }
    approvals_valid(
        unit,
        policy,
        approvals,
        evidence_set_handle,
        approval_set_handle,
        now_ns,
    )
}

fn metrics_valid(
    unit: &SwapUnit,
    policy: &TransitionPolicy,
    evidence: &[MetricEvidence],
    evidence_set_handle: [u8; 32],
    now_ns: i64,
) -> Refusal {
    for item in evidence {
        if item.key != unit.key
            || item.policy_version != unit.policy_version
            || item.policy_digest != unit.policy_digest
            || item.source_epoch_digest != unit.source_epoch_digest
            || item.evidence_set_handle != evidence_set_handle
            || item.measured_stage != policy.from
        {
            return Refusal::EvidenceScopeMismatch;
        }
        let mut known_metric = false;
        for index in 0..usize::from(policy.criterion_count) {
            if let Some(criterion) = policy.criteria.get(index) {
                if criterion.metric_id == item.metric_id {
                    known_metric = true;
                }
            }
        }
        if !known_metric {
            return Refusal::EvidenceScopeMismatch;
        }
        match item.quality {
            EvidenceQuality::Complete => {}
            EvidenceQuality::Incomplete => return Refusal::EvidenceIncomplete,
            EvidenceQuality::Unresolved => return Refusal::EvidenceUnresolved,
        }
        if (item.measured_stage as u8) >= (Stage::S2Window as u8) {
            let (origin_key, activation_ns, receipt_handle) = if unit.has_prior_dependency {
                let prior = unit.prior_dependency;
                if prior.achieved_stage != item.measured_stage {
                    return Refusal::EvidenceScopeMismatch;
                }
                (prior.key, prior.observed_at_ns, prior.artifact_handle)
            } else {
                (unit.key, unit.last_transition_ns, [0; 32])
            };
            if item.observation_origin_key != origin_key
                || item.origin_stage_activation_ns != activation_ns
                || item.origin_receipt_handle != receipt_handle
                || item.observation_start_ns < activation_ns
                || item.observation_start_ns < origin_key.window_start_ns
                || item.observation_end_ns > origin_key.window_end_ns
                || item.observation_end_ns < activation_ns
            {
                return Refusal::EvidenceScopeMismatch;
            }
        }
    }
    for criterion_index in 0..usize::from(policy.criterion_count) {
        let criterion = match policy.criteria.get(criterion_index) {
            Some(value) => value,
            None => return Refusal::InvalidPolicy,
        };
        for earlier_index in 0..criterion_index {
            if let Some(earlier) = policy.criteria.get(earlier_index) {
                if earlier.metric_id == criterion.metric_id {
                    return Refusal::InvalidPolicy;
                }
            }
        }
        let mut found = false;
        for item in evidence {
            if item.metric_id != criterion.metric_id || item.measured_stage != policy.from {
                continue;
            }
            if item.key != unit.key
                || item.policy_version != unit.policy_version
                || item.policy_digest != unit.policy_digest
                || item.source_epoch_digest != unit.source_epoch_digest
            {
                return Refusal::EvidenceScopeMismatch;
            }
            let age = match now_ns.checked_sub(item.observation_end_ns) {
                Some(value) if value >= 0 => match u64::try_from(value) {
                    Ok(converted) => converted,
                    Err(_) => return Refusal::TimeInvalid,
                },
                _ => return Refusal::TimeInvalid,
            };
            let observation_span = match item
                .observation_end_ns
                .checked_sub(item.observation_start_ns)
            {
                Some(span) if span >= 0 => match u64::try_from(span) {
                    Ok(value) => value,
                    Err(_) => return Refusal::TimeInvalid,
                },
                _ => return Refusal::TimeInvalid,
            };
            if item.observation_end_ns > item.observed_at_ns
                || item.observed_at_ns > now_ns
                || item.exposure_ns > observation_span
                || age > policy.maximum_evidence_age_ns
                || !handle_present(&item.artifact_handle)
            {
                return Refusal::StaleEvidence;
            }
            if item.samples > item.denominator
                || item.samples < criterion.minimum_samples
                || item.denominator < criterion.minimum_denominator
                || item.exposure_ns < criterion.minimum_exposure_ns
            {
                return Refusal::MetricFailed;
            }
            let pass = match criterion.comparison {
                Comparison::AtMost => item.observed <= criterion.threshold,
                Comparison::AtLeast => item.observed >= criterion.threshold,
            };
            if !pass {
                return Refusal::MetricFailed;
            }
            if found {
                return Refusal::InvalidPolicy;
            }
            found = true;
        }
        if !found {
            return Refusal::EvidenceMissing;
        }
    }
    Refusal::None
}

fn dependencies_valid(
    unit: &SwapUnit,
    policy: &TransitionPolicy,
    evidence: &[DependencyEvidence],
    now_ns: i64,
) -> Refusal {
    if usize::from(policy.dependency_count) > evidence.len() {
        return Refusal::DependencyMissing;
    }
    if usize::from(policy.dependency_count) != evidence.len() {
        return Refusal::DependencyMismatch;
    }
    if unit.has_prior_dependency {
        let mut found_prior = false;
        for item in evidence {
            if *item == unit.prior_dependency {
                found_prior = true;
            }
        }
        if !found_prior {
            return Refusal::DependencyMismatch;
        }
    }
    for index in 0..usize::from(policy.dependency_count) {
        let requirement = match policy.dependencies.get(index) {
            Some(value) => value,
            None => return Refusal::InvalidPolicy,
        };
        let mut found = false;
        for item in evidence {
            if item.key != requirement.key {
                continue;
            }
            let age = match now_ns.checked_sub(item.observed_at_ns) {
                Some(value) if value >= 0 => value,
                _ => return Refusal::DependencyMismatch,
            };
            if item.quality != EvidenceQuality::Complete
                || (item.achieved_stage as u8) < (requirement.minimum_stage as u8)
                || item.policy_version == 0
                || item.policy_version != requirement.policy_version
                || item.policy_digest != requirement.policy_digest
                || item.source_epoch_digest != requirement.source_epoch_digest
                || !handle_present(&item.artifact_handle)
                || u64::try_from(age).map_or(true, |value| value > policy.maximum_evidence_age_ns)
                || item.observed_at_ns > item.key.window_end_ns
            {
                return Refusal::DependencyMismatch;
            }
            found = true;
        }
        if !found {
            return Refusal::DependencyMissing;
        }
    }
    Refusal::None
}

fn approvals_valid(
    unit: &SwapUnit,
    policy: &TransitionPolicy,
    approvals: &[AuthorityApproval],
    evidence_set_handle: [u8; 32],
    approval_set_handle: [u8; 32],
    now_ns: i64,
) -> Refusal {
    let mut roles = 0u8;
    for approval in approvals {
        if approval.key != unit.key
            || approval.from != policy.from
            || approval.to != policy.to
            || approval.policy_version != unit.policy_version
            || approval.policy_digest != unit.policy_digest
            || approval.source_epoch_digest != unit.source_epoch_digest
            || approval.evidence_set_handle != evidence_set_handle
            || approval.approval_set_handle != approval_set_handle
        {
            continue;
        }
        if approval.approved_at_ns > now_ns {
            return Refusal::TimeInvalid;
        }
        let age = match now_ns.checked_sub(approval.approved_at_ns) {
            Some(value) if value >= 0 => value,
            _ => return Refusal::TimeInvalid,
        };
        if u64::try_from(age).map_or(true, |value| value > policy.maximum_evidence_age_ns) {
            return Refusal::StaleEvidence;
        }
        if !approval.approved {
            return Refusal::AuthorityRejected;
        }
        if approval.principal_id == 0 || !handle_present(&approval.authority_record_handle) {
            continue;
        }
        let role = role_bit(approval.role);
        if roles & role != 0 {
            return Refusal::AuthorityConflict;
        }
        roles |= role;
    }
    let required = policy.required_role_mask | mandatory_roles(policy.to);
    if roles & required != required {
        return if policy.to == Stage::S5Retired && roles & ROLE_RETIREMENT == 0 {
            Refusal::RetirementAuthorityRequired
        } else {
            Refusal::AuthorityMissing
        };
    }
    let assessor = approval_principal_for_role(
        approvals,
        unit.key,
        policy,
        evidence_set_handle,
        approval_set_handle,
        AuthorityRole::IndependentAssessor,
    );
    let operational = if policy.to == Stage::S5Retired {
        approval_principal_for_role(
            approvals,
            unit.key,
            policy,
            evidence_set_handle,
            approval_set_handle,
            AuthorityRole::FacilityRetirementAuthority,
        )
    } else {
        approval_principal_for_role(
            approvals,
            unit.key,
            policy,
            evidence_set_handle,
            approval_set_handle,
            AuthorityRole::FacilitySupervisor,
        )
    };
    if assessor == 0 || assessor == operational {
        return Refusal::AuthorityMissing;
    }
    Refusal::None
}

fn has_minimum_exposure(policy: &TransitionPolicy, required_ns: u64) -> bool {
    for index in 0..usize::from(policy.criterion_count) {
        if let Some(criterion) = policy.criteria.get(index) {
            if criterion.minimum_exposure_ns >= required_ns {
                return true;
            }
        }
    }
    false
}

fn valid_rollback_policy(policy: RollbackTriggerPolicy) -> bool {
    rollback_rule_valid(policy.blocking_difference, ROLE_SUPERVISOR | ROLE_TECH_OPS)
        && rollback_rule_valid(
            policy.controller_request,
            ROLE_SUPERVISOR | ROLE_CONTROLLER_ON_POSITION,
        )
        && rollback_rule_valid(policy.health_fault, ROLE_SUPERVISOR | ROLE_TECH_OPS)
}

fn rollback_rule_valid(rule: RollbackRule, permitted_roles: u8) -> bool {
    rule.enabled
        && rule.maximum_response_ns > 0
        && rule.required_role_mask != 0
        && rule.required_role_mask & !0x3f == 0
        && rule.required_role_mask & permitted_roles != 0
}

fn approval_principal_for_role(
    approvals: &[AuthorityApproval],
    key: SwapUnitKey,
    policy: &TransitionPolicy,
    evidence_set_handle: [u8; 32],
    approval_set_handle: [u8; 32],
    role: AuthorityRole,
) -> u32 {
    for approval in approvals {
        if approval.role == role
            && approval.approved
            && approval.key == key
            && approval.from == policy.from
            && approval.to == policy.to
            && approval.policy_digest == policy.policy_digest
            && approval.policy_version == policy.policy_version
            && approval.source_epoch_digest == policy.source_epoch_digest
            && approval.evidence_set_handle == evidence_set_handle
            && approval.approval_set_handle == approval_set_handle
            && approval.principal_id != 0
            && handle_present(&approval.authority_record_handle)
        {
            return approval.principal_id;
        }
    }
    0
}

const fn mandatory_roles(stage: Stage) -> u8 {
    match stage {
        Stage::S5Retired => ROLE_RETIREMENT | ROLE_ASSESSOR,
        _ => ROLE_SUPERVISOR | ROLE_ASSESSOR,
    }
}

const fn role_bit(role: AuthorityRole) -> u8 {
    1 << role as u8
}

fn approving_principal(
    approvals: &[AuthorityApproval],
    key: SwapUnitKey,
    policy: &TransitionPolicy,
    evidence_set_handle: [u8; 32],
    approval_set_handle: [u8; 32],
) -> u32 {
    let required = if policy.to == Stage::S5Retired {
        ROLE_RETIREMENT
    } else {
        ROLE_SUPERVISOR
    };
    for approval in approvals {
        if role_bit(approval.role) == required
            && approval.approved
            && approval.principal_id != 0
            && handle_present(&approval.authority_record_handle)
            && approval.key == key
            && approval.from == policy.from
            && approval.to == policy.to
            && approval.policy_digest == policy.policy_digest
            && approval.policy_version == policy.policy_version
            && approval.source_epoch_digest == policy.source_epoch_digest
            && approval.evidence_set_handle == evidence_set_handle
            && approval.approval_set_handle == approval_set_handle
        {
            return approval.principal_id;
        }
    }
    0
}

const fn next_stage(stage: Stage) -> Option<Stage> {
    match stage {
        Stage::S0Shadow => Some(Stage::S1Advisory),
        Stage::S1Advisory => Some(Stage::S2Window),
        Stage::S2Window => Some(Stage::S3Extended),
        Stage::S3Extended => Some(Stage::S4Continuous),
        Stage::S4Continuous => Some(Stage::S5Retired),
        Stage::S5Retired => None,
    }
}

const fn valid_key(key: SwapUnitKey) -> bool {
    key.function_id != 0 && key.scope_id != 0 && key.window_start_ns < key.window_end_ns
}

fn handle_present(handle: &[u8; 32]) -> bool {
    let mut index = 0;
    while index < 32 {
        match handle.get(index) {
            Some(value) if *value != 0 => return true,
            Some(_) => {}
            None => return false,
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    const KEY: SwapUnitKey = SwapUnitKey {
        function_id: 1,
        scope_id: 7,
        window_start_ns: 10,
        window_end_ns: 10_000,
    };
    const H: [u8; 32] = [1; 32];

    fn policy(key: SwapUnitKey, from: Stage, to: Stage) -> TransitionPolicy {
        TransitionPolicy {
            key,
            from,
            to,
            policy_version: 1,
            policy_digest: H,
            source_epoch_digest: H,
            maximum_evidence_age_ns: 10_000,
            required_role_mask: ROLE_SUPERVISOR | ROLE_ASSESSOR,
            criterion_count: 1,
            criteria: [MetricCriterion {
                metric_id: 4,
                comparison: Comparison::AtMost,
                threshold: 3,
                minimum_samples: 1,
                minimum_denominator: 1,
                minimum_exposure_ns: 1,
            }; MAX_CRITERIA],
            dependency_count: 0,
            dependencies: [DependencyRequirement {
                key,
                minimum_stage: Stage::S0Shadow,
                policy_version: 1,
                policy_digest: H,
                source_epoch_digest: H,
            }; MAX_DEPENDENCIES],
            rollback_policy: RollbackTriggerPolicy {
                blocking_difference: RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000_000_000,
                    required_role_mask: ROLE_SUPERVISOR,
                },
                controller_request: RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000_000_000,
                    required_role_mask: ROLE_CONTROLLER_ON_POSITION,
                },
                health_fault: RollbackRule {
                    enabled: true,
                    maximum_response_ns: 1_000_000_000,
                    required_role_mask: ROLE_TECH_OPS,
                },
            },
        }
    }

    fn evidence(
        key: SwapUnitKey,
        stage: Stage,
        observation_start_ns: i64,
        observation_end_ns: i64,
        observed_at_ns: i64,
    ) -> [MetricEvidence; 1] {
        [MetricEvidence {
            key,
            measured_stage: stage,
            metric_id: 4,
            quality: EvidenceQuality::Complete,
            policy_version: 1,
            samples: 5,
            denominator: 8,
            observed: 2,
            exposure_ns: u64::try_from(observation_end_ns - observation_start_ns).unwrap(),
            observation_start_ns,
            observation_end_ns,
            observed_at_ns,
            policy_digest: H,
            source_epoch_digest: H,
            artifact_handle: H,
            evidence_set_handle: H,
            observation_origin_key: key,
            origin_stage_activation_ns: 0,
            origin_receipt_handle: [0; 32],
        }]
    }

    fn approvals(
        key: SwapUnitKey,
        from: Stage,
        to: Stage,
        approved_at_ns: i64,
    ) -> [AuthorityApproval; 2] {
        [
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::FacilitySupervisor,
                approved: true,
                principal_id: 11,
                approved_at_ns,
                policy_version: 1,
                policy_digest: H,
                source_epoch_digest: H,
                evidence_set_handle: H,
                approval_set_handle: H,
                authority_record_handle: H,
            },
            AuthorityApproval {
                key,
                from,
                to,
                role: AuthorityRole::IndependentAssessor,
                approved: true,
                principal_id: 12,
                approved_at_ns,
                policy_version: 1,
                policy_digest: H,
                source_epoch_digest: H,
                evidence_set_handle: H,
                approval_set_handle: H,
                authority_record_handle: H,
            },
        ]
    }

    fn input<'a>(
        policy: &'a TransitionPolicy,
        metrics: &'a [MetricEvidence],
        dependencies: &'a [DependencyEvidence],
        approvals: &'a [AuthorityApproval],
        now_ns: i64,
    ) -> TransitionInput<'a> {
        TransitionInput {
            policy,
            metrics,
            dependencies,
            approvals,
            evidence_set_handle: H,
            approval_set_handle: H,
            now_ns,
        }
    }

    // REQ: OA-MIG-101
    #[test]
    fn evaluates_scoped_numeric_evidence_and_audit_result() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let ev = evidence(KEY, Stage::S0Shadow, 0, 10, 10);
        let auth = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 100);
        let policy = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let mut audit = empty_audit_log();
        let result = evaluate_transition(&unit, input(&policy, &ev, &[], &auth, 100), &mut audit);
        assert_eq!(result.event.refusal, Refusal::None);
        assert_eq!(result.unit.stage, Stage::S1Advisory);
        assert_eq!(result.unit.audit_sequence, 1);
        assert_eq!(audit.len, 1);
        assert_eq!(audit.entries[0].refusal, Refusal::None);
    }

    // REQ: OA-MIG-101
    #[test]
    fn prior_shadow_evidence_can_qualify_a_half_open_swap_window() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut audit = empty_audit_log();
        let shadow = evidence(KEY, Stage::S0Shadow, 0, 8, 9);
        let s1_approvals = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 9);
        let s1_policy = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let s1 = evaluate_transition(
            &unit,
            input(&s1_policy, &shadow, &[], &s1_approvals, 9),
            &mut audit,
        );
        assert_eq!(s1.event.refusal, Refusal::None);
        assert_eq!(s1.unit.stage, Stage::S1Advisory);

        let advisory = evidence(KEY, Stage::S1Advisory, 9, 10, 10);
        let s2_approvals = approvals(KEY, Stage::S1Advisory, Stage::S2Window, 10);
        let s2_policy = policy(KEY, Stage::S1Advisory, Stage::S2Window);
        let s2 = evaluate_transition(
            &s1.unit,
            input(&s2_policy, &advisory, &[], &s2_approvals, 10),
            &mut audit,
        );
        assert_eq!(s2.event.refusal, Refusal::None);
        assert_eq!(s2.unit.stage, Stage::S2Window);
        assert_eq!(audit.len, 2);
    }

    // REQ: OA-MIG-101
    #[test]
    fn expired_swap_window_and_ambiguous_approvals_refuse() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut audit = empty_audit_log();
        let shadow = evidence(KEY, Stage::S0Shadow, 0, 8, 9);
        let s1_approvals = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 9);
        let s1_policy = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let s1 = evaluate_transition(
            &unit,
            input(&s1_policy, &shadow, &[], &s1_approvals, 9),
            &mut audit,
        );

        let advisory = evidence(KEY, Stage::S1Advisory, 9, 9_990, 9_990);
        let s2_policy = policy(KEY, Stage::S1Advisory, Stage::S2Window);
        let valid_approvals = approvals(KEY, Stage::S1Advisory, Stage::S2Window, 9_990);
        let expired = evaluate_transition(
            &s1.unit,
            input(
                &s2_policy,
                &advisory,
                &[],
                &valid_approvals,
                KEY.window_end_ns,
            ),
            &mut audit,
        );
        assert_eq!(expired.event.refusal, Refusal::TimeInvalid);
        assert_eq!(expired.unit.stage, Stage::S1Advisory);

        let mut conflicting = [valid_approvals[0], valid_approvals[1], valid_approvals[0]];
        conflicting[2].principal_id = 99;
        let ambiguous = evaluate_transition(
            &s1.unit,
            input(&s2_policy, &advisory, &[], &conflicting, 9_990),
            &mut audit,
        );
        assert_eq!(ambiguous.event.refusal, Refusal::AuthorityConflict);
        assert_eq!(ambiguous.unit.stage, Stage::S1Advisory);
        let reversed = [conflicting[2], conflicting[1], conflicting[0]];
        let reordered = evaluate_transition(
            &s1.unit,
            input(&s2_policy, &advisory, &[], &reversed, 9_990),
            &mut audit,
        );
        assert_eq!(reordered.event.refusal, ambiguous.event.refusal);
        assert_eq!(reordered.event.principal_id, ambiguous.event.principal_id);
    }

    // REQ: OA-MIG-101
    #[test]
    fn continued_extended_window_requires_prior_unit_receipt() {
        const S3_KEY: SwapUnitKey = SwapUnitKey {
            function_id: 1,
            scope_id: 7,
            window_start_ns: 10_000,
            window_end_ns: 172_800_010_000_000,
        };
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut audit = empty_audit_log();
        let shadow = evidence(KEY, Stage::S0Shadow, 0, 8, 9);
        let auth1 = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 9);
        let policy1 = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let s1 = evaluate_transition(&unit, input(&policy1, &shadow, &[], &auth1, 9), &mut audit);
        let advisory = evidence(KEY, Stage::S1Advisory, 9, 10, 10);
        let auth2 = approvals(KEY, Stage::S1Advisory, Stage::S2Window, 10);
        let policy2 = policy(KEY, Stage::S1Advisory, Stage::S2Window);
        let s2 = evaluate_transition(
            &s1.unit,
            input(&policy2, &advisory, &[], &auth2, 10),
            &mut audit,
        );
        let start = new_unit_continuation(&s2, S3_KEY, 1, H, H, 10_000, H).unwrap();
        let s3_unit = continuation_unit(&start);
        assert_eq!(s3_unit.stage, Stage::S2Window);
        let dependency = continuation_dependency(&start);
        let requirement = dependency_requirement(&dependency, Stage::S2Window);
        let mut policy3 = policy(S3_KEY, Stage::S2Window, Stage::S3Extended);
        policy3.dependency_count = 1;
        policy3.dependencies[0] = requirement;
        let mut observations = evidence(S3_KEY, Stage::S2Window, 10, 9_999, 9_999);
        observations[0].observation_origin_key = dependency.key;
        observations[0].origin_stage_activation_ns = dependency.observed_at_ns;
        observations[0].origin_receipt_handle = dependency.artifact_handle;
        let auth3 = approvals(S3_KEY, Stage::S2Window, Stage::S3Extended, 10_000);
        let s3 = evaluate_transition(
            &s3_unit,
            input(&policy3, &observations, &[dependency], &auth3, 10_000),
            &mut audit,
        );
        assert_eq!(s3.event.refusal, Refusal::None);
        assert_eq!(s3.unit.stage, Stage::S3Extended);

        const S4_KEY: SwapUnitKey = SwapUnitKey {
            function_id: 1,
            scope_id: 7,
            window_start_ns: S3_KEY.window_end_ns,
            window_end_ns: S3_KEY.window_end_ns + S4_MIN_EXPOSURE_NS as i64,
        };
        let s4_start =
            new_unit_continuation(&s3, S4_KEY, 1, H, H, S3_KEY.window_end_ns, H).unwrap();
        let s4_unit = continuation_unit(&s4_start);
        let s3_dependency = continuation_dependency(&s4_start);
        let mut policy4 = policy(S4_KEY, Stage::S3Extended, Stage::S4Continuous);
        policy4.dependency_count = 1;
        policy4.dependencies[0] = dependency_requirement(&s3_dependency, Stage::S3Extended);
        let mut borrowed = evidence(
            S4_KEY,
            Stage::S3Extended,
            s3_dependency.observed_at_ns - S3_MIN_EXPOSURE_NS as i64,
            s3_dependency.observed_at_ns + S3_MIN_EXPOSURE_NS as i64,
            s3_dependency.observed_at_ns + S3_MIN_EXPOSURE_NS as i64,
        );
        borrowed[0].observation_origin_key = s3_dependency.key;
        borrowed[0].origin_stage_activation_ns = s3_dependency.observed_at_ns;
        borrowed[0].origin_receipt_handle = s3_dependency.artifact_handle;
        borrowed[0].exposure_ns = S3_MIN_EXPOSURE_NS;
        policy4.maximum_evidence_age_ns = S4_MIN_EXPOSURE_NS;
        policy4.criteria[0].minimum_exposure_ns = S3_MIN_EXPOSURE_NS;
        let auth4 = approvals(
            S4_KEY,
            Stage::S3Extended,
            Stage::S4Continuous,
            S4_KEY.window_start_ns,
        );
        let refused = evaluate_transition(
            &s4_unit,
            input(
                &policy4,
                &borrowed,
                &[s3_dependency],
                &auth4,
                S4_KEY.window_start_ns,
            ),
            &mut audit,
        );
        assert_eq!(refused.event.refusal, Refusal::EvidenceScopeMismatch);
        assert_eq!(refused.unit.stage, Stage::S3Extended);
    }

    // REQ: OA-MIG-101
    #[test]
    fn rejects_sample_count_above_denominator() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut ev = evidence(KEY, Stage::S0Shadow, 0, 10, 10);
        ev[0].samples = 9;
        let auth = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 100);
        let pol = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let mut audit = empty_audit_log();
        let result = evaluate_transition(&unit, input(&pol, &ev, &[], &auth, 100), &mut audit);
        assert_eq!(result.event.refusal, Refusal::MetricFailed);
        assert_eq!(result.unit.stage, Stage::S0Shadow);
    }

    // REQ: OA-MIG-101
    #[test]
    fn refuses_wrong_scope_and_does_not_advance() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut ev = evidence(KEY, Stage::S0Shadow, 0, 10, 10);
        ev[0].key.scope_id = 8;
        let auth = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 100);
        let policy = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let mut audit = empty_audit_log();
        let result = evaluate_transition(&unit, input(&policy, &ev, &[], &auth, 100), &mut audit);
        assert_eq!(result.event.refusal, Refusal::EvidenceScopeMismatch);
        assert_eq!(result.unit.stage, unit.stage);
        assert_eq!(result.unit.audit_sequence, unit.audit_sequence + 1);
        assert_eq!(audit.len, 1);
        assert_eq!(audit.entries[0].refusal, Refusal::EvidenceScopeMismatch);
    }

    // REQ: OA-MIG-101
    #[test]
    fn unresolved_metric_and_missing_dependency_refuse_closed() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut ev = evidence(KEY, Stage::S0Shadow, 0, 10, 10);
        ev[0].quality = EvidenceQuality::Unresolved;
        let auth = approvals(KEY, Stage::S0Shadow, Stage::S1Advisory, 100);
        let mut pol = policy(KEY, Stage::S0Shadow, Stage::S1Advisory);
        let mut audit = empty_audit_log();
        let result = evaluate_transition(&unit, input(&pol, &ev, &[], &auth, 100), &mut audit);
        assert_eq!(result.event.refusal, Refusal::EvidenceUnresolved);
        assert_eq!(result.unit.stage, Stage::S0Shadow);
        assert_eq!(audit.len, 1);

        pol.dependency_count = 1;
        pol.dependencies[0].key.function_id = 2;
        let ev = evidence(KEY, Stage::S0Shadow, 0, 10, 10);
        let result =
            evaluate_transition(&result.unit, input(&pol, &ev, &[], &auth, 100), &mut audit);
        assert_eq!(result.event.refusal, Refusal::DependencyMissing);
        assert_eq!(result.unit.stage, Stage::S0Shadow);
    }

    // REQ: OA-MIG-101
    #[test]
    fn retirement_requires_facility_authority_and_assessor() {
        let unit = new_unit(KEY, 1, H, H, 0).unwrap();
        let mut continuous = unit;
        continuous.stage = Stage::S4Continuous;
        let prior_window_ns = i64::try_from(S4_MIN_EXPOSURE_NS).unwrap();
        let retirement_key = SwapUnitKey {
            function_id: KEY.function_id,
            scope_id: KEY.scope_id,
            window_start_ns: -prior_window_ns,
            window_end_ns: 100,
        };
        continuous.key = retirement_key;
        let mut pol = policy(retirement_key, Stage::S4Continuous, Stage::S5Retired);
        pol.required_role_mask = ROLE_RETIREMENT | ROLE_ASSESSOR;
        pol.criteria[0].minimum_exposure_ns = S4_MIN_EXPOSURE_NS;
        let observation_start_ns = 10 - prior_window_ns;
        continuous.last_transition_ns = observation_start_ns;
        let mut ev = evidence(
            retirement_key,
            Stage::S4Continuous,
            observation_start_ns,
            10,
            10,
        );
        ev[0].observation_origin_key = retirement_key;
        ev[0].origin_stage_activation_ns = observation_start_ns;
        let mut auth = approvals(retirement_key, Stage::S4Continuous, Stage::S5Retired, 99);
        auth[0].role = AuthorityRole::FacilitySupervisor;
        let mut audit = empty_audit_log();
        let result = evaluate_transition(&continuous, input(&pol, &ev, &[], &auth, 99), &mut audit);
        assert_eq!(result.event.refusal, Refusal::RetirementAuthorityRequired);
        assert_eq!(result.unit.stage, Stage::S4Continuous);
    }
}

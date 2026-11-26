#![forbid(unsafe_code)]
#![no_std]

//! Pure, dependency-free audit event, role and receipt value contract.
//!
//! This crate validates caller-supplied claims and binds a receipt value to an exact event. It
//! cannot establish that a receipt was durably written or who supplied it. The shell log owns
//! canonical encoding, hash verification, and durable I/O; callers must use its receipt result.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("64-bit only");

pub type Digest = [u8; 32];
pub const DETAILS_CAPACITY: usize = 20_480;
pub const TARGET_CAPACITY: usize = 96;

pub mod principal;

pub use principal::{
    authorize_principal, parse_principal_registry, PrincipalAuthorizationError, PrincipalRecord,
    PrincipalRegistry, RegistryError, MAX_PRINCIPAL_REGISTRY_BYTES, MAX_PRINCIPAL_REGISTRY_ROWS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Monitor = 1,
    Operate = 2,
    Configure = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    Verify = 1,
    MigrationFlip = 2,
    MigrationRollback = 3,
    AdaptationIssue = 4,
    Certification = 5,
    QuarantineRelease = 6,
    VulnerabilityIntake = 7,
    VulnerabilityDisposition = 8,
    RunwayAssignment = 9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub principal: u64,
    pub role: Role,
    pub action: Action,
    pub target: [u8; TARGET_CAPACITY],
    pub target_len: u8,
    pub time_ns: u64,
    pub details: [u8; DETAILS_CAPACITY],
    pub details_len: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub sequence: u64,
    pub event: Event,
    pub previous: Digest,
    pub digest: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub count: u64,
    pub tail: Digest,
}

/// Plain value returned after shell append; it is neither a capability nor authenticated proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditReceipt {
    pub sequence: u64,
    pub digest: Digest,
    pub previous: Digest,
    pub event: Event,
}

impl AuditReceipt {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    pub const fn digest(&self) -> Digest {
        self.digest
    }
    pub const fn previous(&self) -> Digest {
        self.previous
    }
    pub const fn principal(&self) -> u64 {
        self.event.principal
    }
    pub const fn role(&self) -> Role {
        self.event.role
    }
    pub const fn action(&self) -> Action {
        self.event.action
    }
    pub fn target(&self) -> &[u8] {
        self.event
            .target
            .get(..usize::from(self.event.target_len))
            .unwrap_or(&[])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditError {
    InvalidEvent,
    RoleDenied,
    SequenceOverflow,
    LogLimitReached,
    InvalidChain,
}

pub const fn genesis() -> Checkpoint {
    Checkpoint {
        count: 0,
        tail: [0; 32],
    }
}

/// The registry checks role claims only; it does not authenticate a principal.
pub const fn authorize(role: Role, action: Action) -> Result<(), AuditError> {
    let allowed = match action {
        Action::Verify => true,
        Action::MigrationFlip
        | Action::MigrationRollback
        | Action::AdaptationIssue
        | Action::Certification => matches!(role, Role::Configure),
        Action::QuarantineRelease | Action::VulnerabilityIntake | Action::RunwayAssignment => {
            matches!(role, Role::Operate | Role::Configure)
        }
        Action::VulnerabilityDisposition => matches!(role, Role::Configure),
    };
    if allowed {
        Ok(())
    } else {
        Err(AuditError::RoleDenied)
    }
}

fn valid_text(bytes: &[u8], len: usize, cap: usize) -> bool {
    len > 0
        && len <= cap
        && bytes
            .get(..len)
            .is_some_and(|s| s.is_ascii() && s.iter().all(|b| (0x20..=0x7e).contains(b)))
}

pub fn validate_event(event: &Event) -> Result<(), AuditError> {
    let target_len = usize::from(event.target_len);
    let details_len = usize::from(event.details_len);
    if event.principal == 0
        || event.time_ns == 0
        || !valid_text(&event.target, target_len, TARGET_CAPACITY)
        || details_len > DETAILS_CAPACITY
        || event
            .target
            .get(target_len..)
            .is_none_or(|tail| tail.iter().any(|b| *b != 0))
        || event
            .details
            .get(details_len..)
            .is_none_or(|tail| tail.iter().any(|b| *b != 0))
    {
        return Err(AuditError::InvalidEvent);
    }
    authorize(event.role, event.action)
}

pub fn target(bytes: &[u8]) -> Result<([u8; TARGET_CAPACITY], u8), AuditError> {
    if !valid_text(bytes, bytes.len(), TARGET_CAPACITY) {
        return Err(AuditError::InvalidEvent);
    }
    let mut out = [0; TARGET_CAPACITY];
    let len = u8::try_from(bytes.len()).map_err(|_| AuditError::InvalidEvent)?;
    out.get_mut(..bytes.len())
        .ok_or(AuditError::InvalidEvent)?
        .copy_from_slice(bytes);
    Ok((out, len))
}

pub fn details(bytes: &[u8]) -> Result<([u8; DETAILS_CAPACITY], u16), AuditError> {
    if bytes.len() > DETAILS_CAPACITY {
        return Err(AuditError::InvalidEvent);
    }
    let mut out = [0; DETAILS_CAPACITY];
    let len = u16::try_from(bytes.len()).map_err(|_| AuditError::InvalidEvent)?;
    out.get_mut(..bytes.len())
        .ok_or(AuditError::InvalidEvent)?
        .copy_from_slice(bytes);
    Ok((out, len))
}

/// Format a digest as the canonical 64-character lowercase hexadecimal target.
pub fn digest_target(digest: &Digest) -> ([u8; TARGET_CAPACITY], u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut target = [0; TARGET_CAPACITY];
    for (index, byte) in digest.iter().copied().enumerate() {
        let Some(offset) = index.checked_mul(2) else {
            return (target, 0);
        };
        let Some(high) = HEX.get(usize::from(byte >> 4)).copied() else {
            return (target, 0);
        };
        let Some(low) = HEX.get(usize::from(byte & 0x0f)).copied() else {
            return (target, 0);
        };
        if let Some(slot) = target.get_mut(offset) {
            *slot = high;
        } else {
            return (target, 0);
        }
        if let Some(slot) = target.get_mut(offset + 1) {
            *slot = low;
        } else {
            return (target, 0);
        }
    }
    (target, 64)
}

/// Build a plain receipt claim from the shell-verified record values.
pub const fn receipt_claim(
    sequence: u64,
    digest: Digest,
    previous: Digest,
    event: Event,
) -> AuditReceipt {
    AuditReceipt {
        sequence,
        digest,
        previous,
        event,
    }
}

/// Bind a receipt value to an exact action and predecessor. Durable persistence is established by
/// the calling shell's append result, not by this pure value comparison.
pub fn receipt_matches(receipt: &AuditReceipt, event: Event) -> bool {
    receipt.sequence > 0 && validate_event(&event).is_ok() && receipt.event == event
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests;

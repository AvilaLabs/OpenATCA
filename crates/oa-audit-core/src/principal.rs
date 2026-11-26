//! Bounded local principal and role registry (E3/I10).
//!
//! This registry validates configured role claims. It is not a login system and does not
//! authenticate a person, credential, origin, or authority.

use crate::{authorize, Action, Role};

pub const PRINCIPAL_REGISTRY_V1_HEADER: &[u8] = b"OACERT-LOCAL-PRINCIPALS-V1\n";
pub const PRINCIPAL_REGISTRY_V2_HEADER: &[u8] = b"OACERT-LOCAL-PRINCIPALS-V2\n";
pub const MAX_PRINCIPAL_REGISTRY_ROWS: usize = 128;
const MAX_PRINCIPAL_ROW_BYTES: usize = 40;
pub const MAX_PRINCIPAL_REGISTRY_BYTES: usize = 64 * 1024;
const PRINCIPAL_REGISTRY_V1_CRLF_HEADER: &[u8] = b"OACERT-LOCAL-PRINCIPALS-V1\r\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    TooLarge,
    BadHeaderOrFraming,
    Empty,
    TooManyPrincipals,
    InvalidPrincipalId,
    DuplicatePrincipal,
    UnknownRole,
    InvalidStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PrincipalRecord {
    pub principal_id: u64,
    pub role: Role,
    pub enabled: bool,
}

const EMPTY_RECORD: PrincipalRecord = PrincipalRecord {
    principal_id: 0,
    role: Role::Monitor,
    enabled: false,
};

/// Cold configuration parsed without allocation. The fixed table is 2,056 bytes on the
/// supported 64-bit target; it stores at most 128 role rows and never grows dynamically.
// A7.2: cold principal configuration; fixed 128-entry table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct PrincipalRegistry {
    entries: [PrincipalRecord; MAX_PRINCIPAL_REGISTRY_ROWS],
    len: u16,
}

const _: () = assert!(core::mem::size_of::<PrincipalRecord>() == 16);
const _: () = assert!(core::mem::size_of::<PrincipalRegistry>() == 2_056);

impl PrincipalRegistry {
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns an entry whether enabled or disabled, so callers can report explicit status.
    pub fn lookup(&self, principal_id: u64) -> Option<PrincipalRecord> {
        let entries = self.entries.get(..self.len())?;
        entries
            .binary_search_by_key(&principal_id, |entry| entry.principal_id)
            .ok()
            .and_then(|index| entries.get(index).copied())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalAuthorizationError {
    UnknownPrincipal,
    DisabledPrincipal,
    RoleDenied,
}

/// Authorizes a configured, enabled principal for an action using the three declared roles.
/// This checks only the local registry's claim; it does not authenticate the caller.
pub fn authorize_principal(
    registry: &PrincipalRegistry,
    principal_id: u64,
    action: Action,
) -> Result<Role, PrincipalAuthorizationError> {
    let principal = registry
        .lookup(principal_id)
        .ok_or(PrincipalAuthorizationError::UnknownPrincipal)?;
    if !principal.enabled {
        return Err(PrincipalAuthorizationError::DisabledPrincipal);
    }
    authorize(principal.role, action).map_err(|_| PrincipalAuthorizationError::RoleDenied)?;
    Ok(principal.role)
}

/// Parses the exact canonical registry formats:
///
/// ```text
/// OACERT-LOCAL-PRINCIPALS-V1\n
/// <positive-u64-decimal>\t<monitor|operate|configure>\n
/// ```
///
/// Version 1 is compatible with the local I1 configure-claim grammar; those rows are enabled.
/// Version 2 adds an explicit status column:
///
/// ```text
/// OACERT-LOCAL-PRINCIPALS-V2\n
/// <positive-u64-decimal>\t<monitor|operate|configure>\t<enabled|disabled>\n
/// ```
///
/// Decimal values have no leading zeroes and every row uses the declared tab-separated columns.
/// Version 1 accepts the same blank-line, optional final-LF, and CRLF forms as its existing
/// caller. Version 2 requires LF-terminated rows and rejects blank rows. Duplicate IDs, extra
/// columns, comments, and unknown tokens are refused. The fixed table bounds both work and memory.
pub fn parse_principal_registry(bytes: &[u8]) -> Result<PrincipalRegistry, RegistryError> {
    if bytes.len() > MAX_PRINCIPAL_REGISTRY_BYTES {
        return Err(RegistryError::TooLarge);
    }
    let (header, explicit_status, allow_blank_rows) =
        if bytes.starts_with(PRINCIPAL_REGISTRY_V1_HEADER) {
            (PRINCIPAL_REGISTRY_V1_HEADER, false, true)
        } else if bytes.starts_with(PRINCIPAL_REGISTRY_V1_CRLF_HEADER) {
            (PRINCIPAL_REGISTRY_V1_CRLF_HEADER, false, true)
        } else if bytes.starts_with(PRINCIPAL_REGISTRY_V2_HEADER) {
            (PRINCIPAL_REGISTRY_V2_HEADER, true, false)
        } else {
            return Err(RegistryError::BadHeaderOrFraming);
        };
    if explicit_status && !bytes.ends_with(b"\n") {
        return Err(RegistryError::BadHeaderOrFraming);
    }
    if bytes.len() == header.len() {
        return Err(RegistryError::Empty);
    }

    let mut registry = PrincipalRegistry {
        entries: [EMPTY_RECORD; MAX_PRINCIPAL_REGISTRY_ROWS],
        len: 0,
    };
    let mut cursor = header.len();
    while cursor < bytes.len() {
        let remaining = bytes
            .get(cursor..)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        let line_terminator = remaining.iter().position(|byte| *byte == b'\n');
        if explicit_status && line_terminator.is_none() {
            return Err(RegistryError::BadHeaderOrFraming);
        }
        let line_len = line_terminator.unwrap_or(remaining.len());
        if line_len > MAX_PRINCIPAL_ROW_BYTES || (line_len == 0 && !allow_blank_rows) {
            return Err(RegistryError::BadHeaderOrFraming);
        }
        if line_len == 0 {
            cursor = cursor
                .checked_add(1)
                .ok_or(RegistryError::BadHeaderOrFraming)?;
            continue;
        }
        let mut line = remaining
            .get(..line_len)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        if !explicit_status && line.last() == Some(&b'\r') {
            line = line
                .get(..line.len().saturating_sub(1))
                .ok_or(RegistryError::BadHeaderOrFraming)?;
        }
        if line.is_empty() {
            if !allow_blank_rows {
                return Err(RegistryError::BadHeaderOrFraming);
            }
            cursor = cursor
                .checked_add(line_len)
                .and_then(|end| end.checked_add(1))
                .ok_or(RegistryError::BadHeaderOrFraming)?;
            continue;
        }
        let (id_bytes, role_bytes, status_bytes) = parse_columns(line, explicit_status)?;
        let principal_id = parse_principal_id(id_bytes)?;
        let current = registry
            .entries
            .get(..registry.len())
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        if current
            .iter()
            .any(|entry| entry.principal_id == principal_id)
        {
            return Err(RegistryError::DuplicatePrincipal);
        }
        let role = parse_role(role_bytes)?;
        let enabled = match status_bytes {
            None => true,
            Some(status) if status == b"enabled" => true,
            Some(status) if status == b"disabled" => false,
            Some(_) => return Err(RegistryError::InvalidStatus),
        };
        let index = current
            .iter()
            .position(|entry| entry.principal_id > principal_id)
            .unwrap_or(registry.len());
        if index < registry.len() {
            let destination = index
                .checked_add(1)
                .ok_or(RegistryError::TooManyPrincipals)?;
            registry
                .entries
                .copy_within(index..registry.len(), destination);
        }
        let slot = registry
            .entries
            .get_mut(index)
            .ok_or(RegistryError::TooManyPrincipals)?;
        *slot = PrincipalRecord {
            principal_id,
            role,
            enabled,
        };
        registry.len = registry
            .len
            .checked_add(1)
            .ok_or(RegistryError::TooManyPrincipals)?;
        cursor = cursor
            .checked_add(line_len)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        if line_terminator.is_some() {
            cursor = cursor
                .checked_add(1)
                .ok_or(RegistryError::BadHeaderOrFraming)?;
        }
    }
    if registry.is_empty() {
        return Err(RegistryError::Empty);
    }
    Ok(registry)
}

fn parse_columns(
    line: &[u8],
    explicit_status: bool,
) -> Result<(&[u8], &[u8], Option<&[u8]>), RegistryError> {
    let first_tab = line
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or(RegistryError::BadHeaderOrFraming)?;
    let after_first = first_tab
        .checked_add(1)
        .ok_or(RegistryError::BadHeaderOrFraming)?;
    let rest = line
        .get(after_first..)
        .ok_or(RegistryError::BadHeaderOrFraming)?;
    let id = line
        .get(..first_tab)
        .ok_or(RegistryError::BadHeaderOrFraming)?;
    let (role, status) = if explicit_status {
        let second_tab_offset = rest
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        let second_tab = after_first
            .checked_add(second_tab_offset)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        let status_start = second_tab
            .checked_add(1)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        let role = line
            .get(after_first..second_tab)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        let status = line
            .get(status_start..)
            .ok_or(RegistryError::BadHeaderOrFraming)?;
        if status.contains(&b'\t') {
            return Err(RegistryError::BadHeaderOrFraming);
        }
        (role, Some(status))
    } else {
        if rest.contains(&b'\t') {
            return Err(RegistryError::BadHeaderOrFraming);
        }
        (rest, None)
    };
    if id.is_empty() || role.is_empty() || status.is_some_and(|value| value.is_empty()) {
        return Err(RegistryError::BadHeaderOrFraming);
    }
    Ok((id, role, status))
}

fn parse_principal_id(bytes: &[u8]) -> Result<u64, RegistryError> {
    if bytes.is_empty()
        || bytes.len() > 20
        || (bytes.len() > 1 && bytes.first() == Some(&b'0'))
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return Err(RegistryError::InvalidPrincipalId);
    }
    let mut value = 0_u64;
    for byte in bytes {
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(u64::from(*byte - b'0')))
            .ok_or(RegistryError::InvalidPrincipalId)?;
    }
    if value == 0 {
        return Err(RegistryError::InvalidPrincipalId);
    }
    Ok(value)
}

fn parse_role(bytes: &[u8]) -> Result<Role, RegistryError> {
    match bytes {
        b"monitor" => Ok(Role::Monitor),
        b"operate" => Ok(Role::Operate),
        b"configure" => Ok(Role::Configure),
        _ => Err(RegistryError::UnknownRole),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &[u8] = b"OACERT-LOCAL-PRINCIPALS-V2\n1\tmonitor\tenabled\n2\toperate\tenabled\n3\tconfigure\tdisabled\n";

    // REQ: OA-AUDIT-007
    #[test]
    fn local_registry_looks_up_enabled_and_disabled_principals_by_exact_id() {
        let registry = parse_principal_registry(REGISTRY).unwrap();
        assert_eq!(registry.len(), 3);
        assert_eq!(
            registry.lookup(1),
            Some(PrincipalRecord {
                principal_id: 1,
                role: Role::Monitor,
                enabled: true,
            })
        );
        assert_eq!(registry.lookup(99), None);
        assert_eq!(
            authorize_principal(&registry, 1, Action::Verify),
            Ok(Role::Monitor)
        );
        assert_eq!(
            authorize_principal(&registry, 2, Action::QuarantineRelease),
            Ok(Role::Operate)
        );
        assert_eq!(
            authorize_principal(&registry, 3, Action::Certification),
            Err(PrincipalAuthorizationError::DisabledPrincipal)
        );
        assert_eq!(
            authorize_principal(&registry, 1, Action::MigrationFlip),
            Err(PrincipalAuthorizationError::RoleDenied)
        );
        assert_eq!(
            authorize_principal(&registry, 99, Action::Verify),
            Err(PrincipalAuthorizationError::UnknownPrincipal)
        );
    }

    // REQ: OA-AUDIT-007
    #[test]
    fn local_registry_v1_is_compatible_with_existing_i1_configure_claim_rows() {
        let registry = parse_principal_registry(
            b"OACERT-LOCAL-PRINCIPALS-V1\r\n12\tmonitor\r\n\r\n11\tconfigure",
        )
        .unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(
            authorize_principal(&registry, 11, Action::Certification),
            Ok(Role::Configure)
        );
        assert_eq!(
            authorize_principal(&registry, 12, Action::MigrationFlip),
            Err(PrincipalAuthorizationError::RoleDenied)
        );
    }

    // REQ: OA-AUDIT-007
    #[test]
    fn local_registry_rejects_noncanonical_ids_roles_statuses_and_row_order() {
        for (invalid, expected) in [
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n0\tmonitor\tenabled\n".as_slice(),
                RegistryError::InvalidPrincipalId,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n01\tmonitor\tenabled\n".as_slice(),
                RegistryError::InvalidPrincipalId,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n18446744073709551616\tmonitor\tenabled\n".as_slice(),
                RegistryError::InvalidPrincipalId,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n1\tadmin\tenabled\n".as_slice(),
                RegistryError::UnknownRole,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n1\tmonitor\tpaused\n".as_slice(),
                RegistryError::InvalidStatus,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n1\tmonitor\tenabled\n1\toperate\tenabled\n"
                    .as_slice(),
                RegistryError::DuplicatePrincipal,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n1\tmonitor\tenabled\textra\n".as_slice(),
                RegistryError::BadHeaderOrFraming,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\r\n1\tmonitor\tenabled\r\n".as_slice(),
                RegistryError::BadHeaderOrFraming,
            ),
            (
                b"OACERT-LOCAL-PRINCIPALS-V2\n1\tmonitor\tenabled".as_slice(),
                RegistryError::BadHeaderOrFraming,
            ),
        ] {
            assert_eq!(parse_principal_registry(invalid), Err(expected));
        }
        assert_eq!(
            parse_principal_registry(b"OACERT-LOCAL-PRINCIPALS-V2\n"),
            Err(RegistryError::Empty)
        );
    }

    // REQ: OA-AUDIT-007
    #[test]
    fn local_registry_row_and_byte_limits_refuse_without_truncation() {
        let too_large = [0_u8; MAX_PRINCIPAL_REGISTRY_BYTES + 1];
        assert_eq!(
            parse_principal_registry(&too_large),
            Err(RegistryError::TooLarge)
        );

        let mut rows = [0_u8; MAX_PRINCIPAL_REGISTRY_BYTES];
        let mut len = PRINCIPAL_REGISTRY_V2_HEADER.len();
        rows.get_mut(..len)
            .unwrap()
            .copy_from_slice(PRINCIPAL_REGISTRY_V2_HEADER);
        for principal in 1..=MAX_PRINCIPAL_REGISTRY_ROWS + 1 {
            let mut value = principal as u64;
            let mut digits = [0_u8; 20];
            let mut digit_count = 0;
            while value > 0 {
                let digit = u8::try_from(value % 10).unwrap();
                if let Some(slot) = digits.get_mut(digit_count) {
                    *slot = b'0' + digit;
                }
                digit_count += 1;
                value /= 10;
            }
            for digit in digits.get(..digit_count).unwrap().iter().rev() {
                if let Some(slot) = rows.get_mut(len) {
                    *slot = *digit;
                }
                len += 1;
            }
            let part = b"\tmonitor\tenabled\n";
            let end = len + part.len();
            rows.get_mut(len..end).unwrap().copy_from_slice(part);
            len = end;
        }
        assert_eq!(
            parse_principal_registry(rows.get(..len).unwrap()),
            Err(RegistryError::TooManyPrincipals)
        );
    }
}

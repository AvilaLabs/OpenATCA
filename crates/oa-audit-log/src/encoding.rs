//! Canonical fixed-width record encoding and hash chaining; shell-only by F1/F6.
use oa_audit_core::{
    validate_event, Action, AuditError, Checkpoint, Event, Record, Role, DETAILS_CAPACITY,
    TARGET_CAPACITY,
};
use oa_sha256::core::sha256::sha256;

const DOMAIN: &[u8] = b"OA-AUDIT-CHAIN-V1\0";
pub const MAX_RECORDS: u64 = 65_536;
pub const RECORD_BYTES: usize =
    DOMAIN.len() + 1 + 8 + 8 + 8 + 1 + 1 + 1 + TARGET_CAPACITY + 2 + DETAILS_CAPACITY + 32 + 32;

pub(super) fn chain(checkpoint: Checkpoint, event: Event) -> Result<Record, AuditError> {
    validate_event(&event)?;
    if checkpoint.count >= MAX_RECORDS {
        return Err(AuditError::LogLimitReached);
    }
    let sequence = checkpoint
        .count
        .checked_add(1)
        .ok_or(AuditError::SequenceOverflow)?;
    let record = Record {
        sequence,
        event,
        previous: checkpoint.tail,
        digest: [0; 32],
    };
    let mut bytes = [0u8; RECORD_BYTES];
    encode_without_hash(&record, &mut bytes)?;
    let digest = sha256(&bytes);
    Ok(Record { digest, ..record })
}

pub(super) fn checkpoint(record: &Record) -> Checkpoint {
    Checkpoint {
        count: record.sequence,
        tail: record.digest,
    }
}

pub(super) fn encode(record: &Record, out: &mut [u8; RECORD_BYTES]) -> Result<(), AuditError> {
    let expected = chain(
        Checkpoint {
            count: record
                .sequence
                .checked_sub(1)
                .ok_or(AuditError::InvalidChain)?,
            tail: record.previous,
        },
        record.event,
    )?;
    if expected.sequence != record.sequence || expected.digest != record.digest {
        return Err(AuditError::InvalidChain);
    }
    encode_without_hash(record, out)?;
    let start = RECORD_BYTES
        .checked_sub(32)
        .ok_or(AuditError::InvalidChain)?;
    out.get_mut(start..RECORD_BYTES)
        .ok_or(AuditError::InvalidChain)?
        .copy_from_slice(&record.digest);
    Ok(())
}

pub(super) fn decode(bytes: &[u8; RECORD_BYTES]) -> Result<Record, AuditError> {
    if bytes.get(..DOMAIN.len()) != Some(DOMAIN) {
        return Err(AuditError::InvalidChain);
    }
    let mut pos = DOMAIN.len();
    let version = read_u8(bytes, &mut pos)?;
    if version != 1 {
        return Err(AuditError::InvalidChain);
    }
    let sequence = read_u64(bytes, &mut pos)?;
    let time_ns = read_u64(bytes, &mut pos)?;
    let principal = read_u64(bytes, &mut pos)?;
    let role = match read_u8(bytes, &mut pos)? {
        1 => Role::Monitor,
        2 => Role::Operate,
        3 => Role::Configure,
        _ => return Err(AuditError::InvalidChain),
    };
    let action = match read_u8(bytes, &mut pos)? {
        1 => Action::Verify,
        2 => Action::MigrationFlip,
        3 => Action::MigrationRollback,
        4 => Action::AdaptationIssue,
        5 => Action::Certification,
        6 => Action::QuarantineRelease,
        _ => return Err(AuditError::InvalidChain),
    };
    let target_len = read_u8(bytes, &mut pos)?;
    let mut target = [0; TARGET_CAPACITY];
    take(bytes, &mut pos, &mut target)?;
    let details_len = read_u16(bytes, &mut pos)?;
    let mut details = [0; DETAILS_CAPACITY];
    take(bytes, &mut pos, &mut details)?;
    let mut previous = [0; 32];
    take(bytes, &mut pos, &mut previous)?;
    let mut digest = [0; 32];
    take(bytes, &mut pos, &mut digest)?;
    let record = Record {
        sequence,
        event: Event {
            principal,
            role,
            action,
            target,
            target_len,
            time_ns,
            details,
            details_len,
        },
        previous,
        digest,
    };
    let mut canonical = [0; RECORD_BYTES];
    encode(&record, &mut canonical)?;
    if canonical != *bytes {
        return Err(AuditError::InvalidChain);
    }
    Ok(record)
}

fn encode_without_hash(record: &Record, out: &mut [u8; RECORD_BYTES]) -> Result<(), AuditError> {
    validate_event(&record.event)?;
    let mut pos = 0;
    put_slice(out, &mut pos, DOMAIN)?;
    put_u8(out, &mut pos, 1)?;
    put_u64(out, &mut pos, record.sequence)?;
    put_u64(out, &mut pos, record.event.time_ns)?;
    put_u64(out, &mut pos, record.event.principal)?;
    put_u8(out, &mut pos, record.event.role as u8)?;
    put_u8(out, &mut pos, record.event.action as u8)?;
    put_u8(out, &mut pos, record.event.target_len)?;
    put(out, &mut pos, &record.event.target)?;
    put_u16(out, &mut pos, record.event.details_len)?;
    put(out, &mut pos, &record.event.details)?;
    put(out, &mut pos, &record.previous)?;
    put(out, &mut pos, &[0; 32])?;
    Ok(())
}

fn take<const N: usize>(src: &[u8], pos: &mut usize, out: &mut [u8; N]) -> Result<(), AuditError> {
    let end = pos.checked_add(N).ok_or(AuditError::InvalidChain)?;
    out.copy_from_slice(src.get(*pos..end).ok_or(AuditError::InvalidChain)?);
    *pos = end;
    Ok(())
}
fn read_u8(src: &[u8], pos: &mut usize) -> Result<u8, AuditError> {
    let mut b = [0];
    take(src, pos, &mut b)?;
    Ok(b[0])
}
fn read_u16(src: &[u8], pos: &mut usize) -> Result<u16, AuditError> {
    let mut b = [0; 2];
    take(src, pos, &mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u64(src: &[u8], pos: &mut usize) -> Result<u64, AuditError> {
    let mut b = [0; 8];
    take(src, pos, &mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn put<const N: usize>(out: &mut [u8], pos: &mut usize, bytes: &[u8; N]) -> Result<(), AuditError> {
    let end = pos.checked_add(N).ok_or(AuditError::InvalidChain)?;
    out.get_mut(*pos..end)
        .ok_or(AuditError::InvalidChain)?
        .copy_from_slice(bytes);
    *pos = end;
    Ok(())
}
fn put_slice(out: &mut [u8], pos: &mut usize, bytes: &[u8]) -> Result<(), AuditError> {
    let end = pos
        .checked_add(bytes.len())
        .ok_or(AuditError::InvalidChain)?;
    out.get_mut(*pos..end)
        .ok_or(AuditError::InvalidChain)?
        .copy_from_slice(bytes);
    *pos = end;
    Ok(())
}
fn put_u8(out: &mut [u8], pos: &mut usize, value: u8) -> Result<(), AuditError> {
    put(out, pos, &[value])
}
fn put_u16(out: &mut [u8], pos: &mut usize, value: u16) -> Result<(), AuditError> {
    put(out, pos, &u16::to_le_bytes(value))
}
fn put_u64(out: &mut [u8], pos: &mut usize, value: u64) -> Result<(), AuditError> {
    put(out, pos, &u64::to_le_bytes(value))
}

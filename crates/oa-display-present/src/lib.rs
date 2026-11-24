//! Direct Linux presentation boundary. All device access is shell code; the contract and
//! event decoder are pure. No desktop, window system, or toolkit is involved.

#![deny(unsafe_code)]
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-display-present supports only the Linux 64-bit UAPI");
#[cfg(not(target_os = "linux"))]
compile_error!("oa-display-present targets Linux only");
#[cfg(target_endian = "big")]
compile_error!("oa-display-present requires little-endian XRGB8888 pixels");

mod linux;

pub use linux::{open_evdev, open_kms, EvdevInput, InputDeviceError, KmsDisplay, KmsError};

/// Maximum dimensions accepted from a mode or caller configuration.
pub const MAX_WIDTH: u32 = 4096;
pub const MAX_HEIGHT: u32 = 2160;
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const INPUT_BATCH_CAPACITY: usize = 128;
pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelFrame<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLayout {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentError {
    InvalidDimensions,
    InvalidStride,
    BufferTooSmall,
    BufferTooLarge,
    UnsupportedFormat,
    UnsupportedMode,
    DeviceUnavailable,
    NotActive,
    System(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KmsLifecycle { Prepared, Active, FlipPending, ReleasePending, ReleasePendingFlip,
    ReleaseAckPending, Suspended, Acquiring, FaultedReleaseAckPending, Faulted, Closed }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmsTransition { Start, FlipSubmitted, FlipCompleted, ReleaseSignal, RestoreSucceeded,
    RestoreFailed, ReleaseDeadlineExpired, ReleaseAcknowledged, AcquireSignal, AcquireSucceeded,
    DropMasterFailed, ReleaseAckFailed, CloseRestored, Close }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleError;

/// Pure owner-process state transition table, also used by the Linux KMS shell.
pub const fn kms_transition(state: KmsLifecycle, event: KmsTransition) -> Result<KmsLifecycle, LifecycleError> {
    use KmsLifecycle as S; use KmsTransition as E;
    match (state, event) {
        (S::Prepared, E::Start) => Ok(S::Active),
        (S::Active, E::FlipSubmitted) => Ok(S::FlipPending),
        (S::FlipPending, E::FlipCompleted) => Ok(S::Active),
        (S::Prepared, E::ReleaseSignal) => Ok(S::ReleasePending),
        (S::Active, E::ReleaseSignal) => Ok(S::ReleasePending),
        (S::FlipPending, E::ReleaseSignal) => Ok(S::ReleasePendingFlip),
        (S::ReleasePendingFlip, E::FlipCompleted) => Ok(S::ReleasePending),
        (S::ReleasePendingFlip, E::ReleaseDeadlineExpired) => Ok(S::FaultedReleaseAckPending),
        (S::ReleasePending, E::RestoreSucceeded) => Ok(S::ReleaseAckPending),
        (S::ReleasePending, E::RestoreFailed) => Ok(S::FaultedReleaseAckPending),
        (S::ReleaseAckPending, E::ReleaseAcknowledged) => Ok(S::Suspended),
        (S::ReleaseAckPending, E::DropMasterFailed) | (S::ReleaseAckPending, E::ReleaseAckFailed) => Ok(S::FaultedReleaseAckPending),
        (S::FaultedReleaseAckPending, E::ReleaseAcknowledged) => Ok(S::Faulted),
        (S::Suspended, E::AcquireSignal) => Ok(S::Acquiring),
        (S::Acquiring, E::AcquireSucceeded) => Ok(S::Active),
        (S::Active, E::CloseRestored) => Ok(S::Closed),
        (S::Prepared, E::Close) | (S::Suspended, E::Close) | (S::Faulted, E::Close) => Ok(S::Closed),
        _ => Err(LifecycleError),
    }
}
/// Whether cleanup may unmap/remove scanout objects without racing a pending flip or active CRTC.
pub const fn kms_cleanup_may_destroy(state: KmsLifecycle, flip_pending: bool, crtc_restored: bool) -> bool {
    !flip_pending && crtc_restored && matches!(state, KmsLifecycle::Prepared | KmsLifecycle::Active | KmsLifecycle::Suspended)
}
const _: () = assert!(core::mem::size_of::<KmsLifecycle>() == 1);

/// Validate complete XRGB8888 frame bounds before any device or buffer access.
pub fn validate_frame(frame: PixelFrame<'_>) -> Result<FrameLayout, PresentError> {
    if frame.width == 0 || frame.height == 0 || frame.width > MAX_WIDTH || frame.height > MAX_HEIGHT {
        return Err(PresentError::InvalidDimensions);
    }
    let row = frame.width.checked_mul(4).ok_or(PresentError::InvalidStride)?;
    if frame.stride < row || frame.stride % 4 != 0 {
        return Err(PresentError::InvalidStride);
    }
    let bytes = usize::try_from(frame.stride).ok()
        .and_then(|s| usize::try_from(frame.height).ok().and_then(|h| s.checked_mul(h)))
        .ok_or(PresentError::BufferTooLarge)?;
    if bytes > MAX_FRAME_BYTES { return Err(PresentError::BufferTooLarge); }
    if frame.pixels.len() < bytes { return Err(PresentError::BufferTooSmall); }
    Ok(FrameLayout { width: frame.width, height: frame.height, stride: frame.stride, bytes })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct InputEvent {
    pub seconds: i64,
    pub microseconds: i64,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}
// Budget: fixed 64-bit evdev record copied by each shell read.
const _: () = assert!(core::mem::size_of::<InputEvent>() == 24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InputRecordKind { Event, SyncReport, SyncLost, Discarded, Resynchronized }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct InputSnapshot {
    pub key_bits: [u8; 96],
    pub absolute_axis_bits: [u8; 8],
    pub absolute_axis_values: [i32; 64],
    pub discarded_records: u32,
    pub relative_motion_lost: u8,
}
const _: () = assert!(core::mem::size_of::<InputSnapshot>() == 368);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InputRecord { pub kind: InputRecordKind, pub event: InputEvent, pub snapshot: InputSnapshot }
// Budget: event plus fixed key snapshot and C alignment padding.
const _: () = assert!(core::mem::size_of::<InputRecord>() == 400);

/// Only a non-repeat EV_KEY press is an action edge. Value 2 is kernel key repeat.
pub const fn input_event_is_action_press(event: InputEvent) -> bool { event.kind == EV_KEY && event.value == 1 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError { Truncated, InvalidTimestamp }

/// Decode the Linux 64-bit `struct input_event` wire layout (24 bytes).
/// SYN_REPORT (`EV_SYN`/`SYN_REPORT`) is represented by `Ok(None)`; all other types are preserved.
pub fn decode_evdev_event(bytes: &[u8]) -> Result<Option<InputEvent>, InputError> {
    if bytes.len() != 24 { return Err(InputError::Truncated); }
    let seconds = i64::from_ne_bytes(bytes.get(0..8).ok_or(InputError::Truncated)?.try_into().map_err(|_| InputError::Truncated)?);
    let microseconds = i64::from_ne_bytes(bytes.get(8..16).ok_or(InputError::Truncated)?.try_into().map_err(|_| InputError::Truncated)?);
    if seconds < 0 || !(0..1_000_000).contains(&microseconds) { return Err(InputError::InvalidTimestamp); }
    let kind = u16::from_ne_bytes(bytes.get(16..18).ok_or(InputError::Truncated)?.try_into().map_err(|_| InputError::Truncated)?);
    let code = u16::from_ne_bytes(bytes.get(18..20).ok_or(InputError::Truncated)?.try_into().map_err(|_| InputError::Truncated)?);
    let value = i32::from_ne_bytes(bytes.get(20..24).ok_or(InputError::Truncated)?.try_into().map_err(|_| InputError::Truncated)?);
    if kind == 0 && code == 0 { return Ok(None); }
    Ok(Some(InputEvent { seconds, microseconds, kind, code, value }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct InputBatch {
    pub seconds: [i64; INPUT_BATCH_CAPACITY],
    pub microseconds: [i64; INPUT_BATCH_CAPACITY],
    pub kinds: [u16; INPUT_BATCH_CAPACITY],
    pub codes: [u16; INPUT_BATCH_CAPACITY],
    pub values: [i32; INPUT_BATCH_CAPACITY],
    pub len: u16,
    pub refused: u32,
}
// Budget: 128 * 24-byte events plus counters and C alignment padding.
const _: () = assert!(core::mem::size_of::<InputBatch>() == 3080);

pub const fn input_batch_new() -> InputBatch {
    InputBatch { seconds: [0; INPUT_BATCH_CAPACITY], microseconds: [0; INPUT_BATCH_CAPACITY],
        kinds: [0; INPUT_BATCH_CAPACITY], codes: [0; INPUT_BATCH_CAPACITY],
        values: [0; INPUT_BATCH_CAPACITY], len: 0, refused: 0 }
}

/// Append a record to the fixed struct-of-arrays batch; a full batch is counted and refused.
pub fn input_batch_push(batch: &mut InputBatch, event: InputEvent) -> bool {
    let i = usize::from(batch.len);
    if i >= INPUT_BATCH_CAPACITY { batch.refused = batch.refused.saturating_add(1); return false; }
    let (Some(seconds), Some(microseconds), Some(kinds), Some(codes), Some(values)) =
        (batch.seconds.get_mut(i), batch.microseconds.get_mut(i), batch.kinds.get_mut(i), batch.codes.get_mut(i), batch.values.get_mut(i))
    else { batch.refused = batch.refused.saturating_add(1); return false; };
    *seconds = event.seconds; *microseconds = event.microseconds; *kinds = event.kind; *codes = event.code; *values = event.value;
    batch.len += 1; true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn checks_frame_products_before_use() {
        let p = [0; 64];
        assert_eq!(validate_frame(PixelFrame { pixels: &p, width: 4, height: 4, stride: 16 }).unwrap().bytes, 64);
        assert_eq!(validate_frame(PixelFrame { pixels: &p, width: 4, height: 4, stride: 12 }), Err(PresentError::InvalidStride));
        assert_eq!(validate_frame(PixelFrame { pixels: &p, width: 4, height: 4, stride: 20 }), Err(PresentError::BufferTooSmall));
    }
    #[test] fn decodes_native_evdev_record_and_sync() {
        let mut b = [0u8; 24]; b[8..16].copy_from_slice(&1i64.to_ne_bytes()); b[16..18].copy_from_slice(&1u16.to_ne_bytes()); b[18..20].copy_from_slice(&30u16.to_ne_bytes()); b[20..24].copy_from_slice(&1i32.to_ne_bytes());
        assert_eq!(decode_evdev_event(&b).unwrap().unwrap().value, 1);
        b[16..18].copy_from_slice(&0u16.to_ne_bytes()); assert_eq!(decode_evdev_event(&b), Ok(None));
    }
    #[test] fn kms_lifecycle_requires_completed_flip_before_normal_release() {
        let state = kms_transition(KmsLifecycle::Prepared, KmsTransition::Start).unwrap();
        let state = kms_transition(state, KmsTransition::FlipSubmitted).unwrap();
        let state = kms_transition(state, KmsTransition::ReleaseSignal).unwrap();
        assert_eq!(state, KmsLifecycle::ReleasePendingFlip);
        let state = kms_transition(state, KmsTransition::FlipCompleted).unwrap();
        let state = kms_transition(state, KmsTransition::RestoreSucceeded).unwrap();
        let state = kms_transition(state, KmsTransition::ReleaseAcknowledged).unwrap();
        assert_eq!(state, KmsLifecycle::Suspended);
        let state = kms_transition(state, KmsTransition::AcquireSignal).unwrap();
        assert_eq!(kms_transition(state, KmsTransition::AcquireSucceeded), Ok(KmsLifecycle::Active));
    }
    #[test] fn pending_flip_deadline_faults_without_claiming_restore() {
        let state = kms_transition(KmsLifecycle::Prepared, KmsTransition::Start).unwrap();
        let state = kms_transition(state, KmsTransition::FlipSubmitted).unwrap();
        let state = kms_transition(state, KmsTransition::ReleaseSignal).unwrap();
        assert_eq!(kms_transition(state, KmsTransition::ReleaseDeadlineExpired), Ok(KmsLifecycle::FaultedReleaseAckPending));
        assert_eq!(kms_transition(KmsLifecycle::Active, KmsTransition::ReleaseAcknowledged), Err(LifecycleError));
    }
    #[test] fn cleanup_waits_for_flip_and_successful_crtc_restore() {
        assert!(!kms_cleanup_may_destroy(KmsLifecycle::Active, true, true));
        assert!(!kms_cleanup_may_destroy(KmsLifecycle::Active, false, false));
        assert!(kms_cleanup_may_destroy(KmsLifecycle::Active, false, true));
        assert!(kms_cleanup_may_destroy(KmsLifecycle::Suspended, false, true));
        assert!(!kms_cleanup_may_destroy(KmsLifecycle::Faulted, false, true));
    }
    #[test] fn key_repeat_is_not_an_action_press() {
        let base = InputEvent { seconds: 0, microseconds: 0, kind: EV_KEY, code: 30, value: 1 };
        assert!(input_event_is_action_press(base));
        assert!(!input_event_is_action_press(InputEvent { value: 2, ..base }));
        assert!(!input_event_is_action_press(InputEvent { value: 0, ..base }));
    }
}

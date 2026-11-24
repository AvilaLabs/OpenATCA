//! Direct Linux presentation boundary. All device access is shell code; the contract and
//! event decoder are pure. No desktop, window system, or toolkit is involved.

#![forbid(unsafe_code)]
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-display-present supports only the Linux 64-bit UAPI");

mod os;

pub use os::{open_kms, KmsDisplay};

/// Maximum dimensions accepted from a mode or caller configuration.
pub const MAX_WIDTH: u32 = 4096;
pub const MAX_HEIGHT: u32 = 2160;
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const INPUT_BATCH_CAPACITY: usize = 128;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub seconds: i64,
    pub microseconds: i64,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError { Truncated, InvalidTimestamp, UnsupportedEvent }

/// Decode the Linux 64-bit `struct input_event` wire layout (24 bytes).
/// SYN_REPORT is represented by `Ok(None)`; other event types are preserved verbatim.
pub fn decode_evdev_event(bytes: &[u8]) -> Result<Option<InputEvent>, InputError> {
    if bytes.len() != 24 { return Err(InputError::Truncated); }
    let i64_at = |o| i64::from_ne_bytes([bytes[o],bytes[o+1],bytes[o+2],bytes[o+3],bytes[o+4],bytes[o+5],bytes[o+6],bytes[o+7]]);
    let u16_at = |o| u16::from_ne_bytes([bytes[o], bytes[o+1]]);
    let i32_at = |o| i32::from_ne_bytes([bytes[o],bytes[o+1],bytes[o+2],bytes[o+3]]);
    let seconds = i64_at(0); let microseconds = i64_at(8);
    if seconds < 0 || !(0..1_000_000).contains(&microseconds) { return Err(InputError::InvalidTimestamp); }
    let kind = u16_at(16);
    if kind == 0 { return Ok(None); }
    if !matches!(kind, 1 | 2 | 3 | 4 | 5 | 17 | 18 | 20) { return Err(InputError::UnsupportedEvent); }
    Ok(Some(InputEvent { seconds, microseconds, kind, code: u16_at(18), value: i32_at(20) }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputBatch { pub events: [InputEvent; INPUT_BATCH_CAPACITY], pub len: u16, pub refused: u32 }

impl InputBatch {
    pub const fn new() -> Self { Self { events: [InputEvent { seconds: 0, microseconds: 0, kind: 0, code: 0, value: 0 }; INPUT_BATCH_CAPACITY], len: 0, refused: 0 } }
    pub fn push(&mut self, event: InputEvent) -> bool {
        let i = usize::from(self.len);
        if i >= INPUT_BATCH_CAPACITY { self.refused = self.refused.saturating_add(1); return false; }
        self.events[i] = event; self.len += 1; true
    }
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
}

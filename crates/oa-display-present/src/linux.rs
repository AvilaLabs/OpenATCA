//! Linux KMS and VT shell. Every raw OS operation is isolated here.
//!
//! ABI layouts and ioctl numbers follow Linux UAPI headers `drm.h`, `drm_mode.h`,
//! `drm_fourcc.h`, `vt.h`, and `kd.h`. Only legacy single-plane linear XRGB8888
//! dumb buffers are accepted. No atomic properties, modifiers, leases, or desktop stack.

#![allow(unsafe_code)] // RULESET A0: sole OS syscall/mmap module in the display-presentation crate.

use crate::{decode_evdev_event, validate_frame, InputError, InputEvent, PixelFrame, PresentError, MAX_FRAME_BYTES, MAX_HEIGHT, MAX_WIDTH};
use core::ffi::{c_int, c_ulong, c_void};
use core::ptr::NonNull;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const DRM_IOCTL_BASE: u32 = b'd' as u32;
const IOC_READ: u32 = 2;
const IOC_WRITE: u32 = 1;
const fn ioc(dir: u32, nr: u32, size: usize) -> c_ulong {
    (((dir << 30) | (DRM_IOCTL_BASE << 8) | nr | ((size as u32) << 16)) as c_ulong)
}
const fn drm_iowr(nr: u32, size: usize) -> c_ulong { ioc(IOC_READ | IOC_WRITE, nr, size) }
const fn drm_io(nr: u32) -> c_ulong { ioc(0, nr, 0) }

const GETRESOURCES: c_ulong = drm_iowr(0xA0, size_of::<CardResources>());
const GETCRTC: c_ulong = drm_iowr(0xA1, size_of::<Crtc>());
const SETCRTC: c_ulong = drm_iowr(0xA2, size_of::<Crtc>());
const GETENCODER: c_ulong = drm_iowr(0xA6, size_of::<Encoder>());
const GETCONNECTOR: c_ulong = drm_iowr(0xA7, size_of::<Connector>());
const ADDFB: c_ulong = drm_iowr(0xAE, size_of::<FbCmd>());
const RMFB: c_ulong = drm_iowr(0xAF, size_of::<u32>());
const PAGE_FLIP: c_ulong = drm_iowr(0xB0, size_of::<PageFlip>());
const CREATE_DUMB: c_ulong = drm_iowr(0xB2, size_of::<CreateDumb>());
const MAP_DUMB: c_ulong = drm_iowr(0xB3, size_of::<MapDumb>());
const DESTROY_DUMB: c_ulong = drm_iowr(0xB4, size_of::<DestroyDumb>());
const SET_MASTER: c_ulong = drm_io(0x1e);
const DROP_MASTER: c_ulong = drm_io(0x1f);
const VT_GETMODE: c_ulong = 0x5601;
const VT_GETSTATE: c_ulong = 0x5603;
const KDGETMODE: c_ulong = 0x4B3B;
const KDSETMODE: c_ulong = 0x4B3A;
const KD_TEXT: c_int = 0;
const KD_GRAPHICS: c_int = 1;
const VT_AUTO: u8 = 0;
const CONNECTED: u32 = 1;
const MODE_PREFERRED: u32 = 1 << 3;
const FLIP_EVENT: u32 = 1;
const EVENT_FLIP_COMPLETE: u32 = 2;
const MAX_OBJECTS: usize = 32;
const MAX_MODES: usize = 64;
const WAIT_MS: c_int = 1000;
const MAX_INPUT_WAIT_MS: u32 = 1000;
const MAX_INTERRUPTS: u8 = 8;

#[repr(C)] #[derive(Clone, Copy, Default)] struct ModeInfo {
    clock: u32, hdisplay: u16, hsync_start: u16, hsync_end: u16, htotal: u16, hskew: u16,
    vdisplay: u16, vsync_start: u16, vsync_end: u16, vtotal: u16, vscan: u16,
    vrefresh: u32, flags: u32, kind: u32, name: [u8; 32],
}
#[repr(C)] #[derive(Default)] struct CardResources {
    fb_ptr: u64, crtc_ptr: u64, connector_ptr: u64, encoder_ptr: u64,
    fb_count: u32, crtc_count: u32, connector_count: u32, encoder_count: u32,
    min_width: u32, max_width: u32, min_height: u32, max_height: u32,
}
#[repr(C)] #[derive(Clone, Copy, Default)] struct Crtc {
    connectors_ptr: u64, connector_count: u32, crtc_id: u32, fb_id: u32,
    x: u32, y: u32, gamma_size: u32, mode_valid: u32, mode: ModeInfo,
}
#[repr(C)] #[derive(Default)] struct Encoder { id: u32, kind: u32, crtc_id: u32, possible_crtcs: u32, clones: u32 }
#[repr(C)] #[derive(Default)] struct Connector {
    encoders_ptr: u64, modes_ptr: u64, props_ptr: u64, prop_values_ptr: u64,
    mode_count: u32, prop_count: u32, encoder_count: u32, encoder_id: u32,
    connector_id: u32, connector_type: u32, type_id: u32, connection: u32,
    mm_width: u32, mm_height: u32, subpixel: u32, pad: u32,
}
#[repr(C)] #[derive(Default)] struct FbCmd { fb_id: u32, width: u32, height: u32, pitch: u32, bpp: u32, depth: u32, handle: u32 }
#[repr(C)] #[derive(Default)] struct CreateDumb { height: u32, width: u32, bpp: u32, flags: u32, handle: u32, pitch: u32, size: u64 }
#[repr(C)] #[derive(Default)] struct MapDumb { handle: u32, pad: u32, offset: u64 }
#[repr(C)] #[derive(Default)] struct DestroyDumb { handle: u32 }
#[repr(C)] #[derive(Default)] struct PageFlip { crtc_id: u32, fb_id: u32, flags: u32, reserved: u32, user_data: u64 }
#[repr(C)] #[derive(Default)] struct VtState { active: u16, signal: u16, state: u16 }
#[repr(C)] #[derive(Default)] struct VtMode { mode: u8, waitv: u8, relsig: i16, acqsig: i16, frsig: i16 }
#[repr(C)] #[derive(Default)] struct PollFd { fd: c_int, events: i16, revents: i16 }

const _: () = assert!(size_of::<ModeInfo>() == 68);
const _: () = assert!(size_of::<CardResources>() == 64);
const _: () = assert!(size_of::<Crtc>() == 104);
const _: () = assert!(size_of::<Connector>() == 80);
const _: () = assert!(size_of::<CreateDumb>() == 32);
const _: () = assert!(size_of::<PageFlip>() == 24);

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn poll(fds: *mut PollFd, count: usize, timeout: c_int) -> c_int;
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
}
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const POLLIN: i16 = 1;
const O_NONBLOCK: i32 = 0x800;
const O_CLOEXEC: i32 = 0x80000;
const O_NOCTTY: i32 = 0x100;
const EVIOCGVERSION: c_ulong = ((IOC_READ << 30) | (4 << 16) | (b'E' as u32 << 8) | 0x01) as c_ulong;

#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum KmsError {
    Present(PresentError), InvalidDevicePath, InactiveVt, VtNotText, DrmMasterUnavailable,
    NoConnectedMode, UnsupportedTopology, UnsupportedDimensions, Timeout, MalformedEvent,
    System(i32), CleanupFailed { operation: i32, rollback: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputDeviceError { InvalidPath, Disconnected, TruncatedRecord, WaitLimit, Timeout, Decode(InputError), System(i32) }

/// Bounded Linux evdev reader for one explicitly selected `/dev/input/eventN` node.
pub struct EvdevInput { file: File, record: [u8; 24], filled: usize }
pub fn open_evdev(path: &str) -> Result<EvdevInput, InputDeviceError> {
    let number = path.strip_prefix("/dev/input/event").ok_or(InputDeviceError::InvalidPath)?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) { return Err(InputDeviceError::InvalidPath); }
    let file = OpenOptions::new().read(true).custom_flags(O_NONBLOCK | O_CLOEXEC)
        .open(path).map_err(|e| InputDeviceError::System(e.raw_os_error().unwrap_or(-1)))?;
    let mut version = 0i32;
    // SAFETY: EVIOCGVERSION writes one initialized 32-bit version value to this live pointer.
    let rc = unsafe { ioctl(file.as_raw_fd(), EVIOCGVERSION, &mut version as *mut i32) };
    if rc < 0 { return Err(InputDeviceError::System(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))); }
    Ok(EvdevInput { file, record: [0; 24], filled: 0 })
}
impl EvdevInput {
    /// Read one complete input record within `timeout_ms` (capped at one second).
    /// `Ok(None)` is a consumed SYN_REPORT; all other records are returned unchanged.
    pub fn read_record(&mut self, timeout_ms: u32) -> Result<Option<InputEvent>, InputDeviceError> {
        if timeout_ms > MAX_INPUT_WAIT_MS { return Err(InputDeviceError::WaitLimit); }
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let deadline = Instant::now() + timeout;
        let mut interruptions = 0u8;
        while self.filled < self.record.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = remaining.as_millis().min(i32::MAX as u128) as c_int;
            let mut fds = PollFd { fd: self.file.as_raw_fd(), events: POLLIN, revents: 0 };
            // SAFETY: fds points to one initialized pollfd for the duration of poll.
            let ready = unsafe { poll(&mut fds, 1, millis) };
            if ready == 0 { return Err(InputDeviceError::Timeout); }
            if ready < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions <= MAX_INTERRUPTS { continue; }
                }
                return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1)));
            }
            if fds.revents & POLLIN == 0 { return Err(InputDeviceError::Disconnected); }
            let target = self.record.get_mut(self.filled..).ok_or(InputDeviceError::TruncatedRecord)?;
            match self.file.read(target) {
                Ok(0) if self.filled == 0 => return Err(InputDeviceError::Disconnected),
                Ok(0) => { self.filled = 0; return Err(InputDeviceError::TruncatedRecord); }
                Ok(n) => self.filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions <= MAX_INTERRUPTS { continue; }
                    return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1)));
                }
                Err(e) => return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1))),
            }
        }
        let decoded = decode_evdev_event(&self.record);
        self.filled = 0;
        decoded.map_err(InputDeviceError::Decode)
    }
}

struct Buffer { handle: u32, fb: u32, pitch: u32, size: usize, map: NonNull<u8> }
impl Buffer {
    fn create(fd: c_int, width: u32, height: u32) -> Result<Self, KmsError> {
        let mut create = CreateDumb { width, height, bpp: 32, ..Default::default() };
        call(fd, CREATE_DUMB, &mut create)?;
        let size = usize::try_from(create.size).map_err(|_| KmsError::UnsupportedDimensions)?;
        if size == 0 || size > MAX_FRAME_BYTES || create.pitch < width.checked_mul(4).ok_or(KmsError::UnsupportedDimensions)? {
            let mut destroy = DestroyDumb { handle: create.handle };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(KmsError::UnsupportedDimensions);
        }
        let mapped_extent = match usize::try_from(create.pitch).ok()
            .and_then(|p| usize::try_from(height).ok().and_then(|h| p.checked_mul(h))) {
            Some(extent) => extent,
            None => { let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy); return Err(KmsError::UnsupportedDimensions); }
        };
        if mapped_extent > size {
            let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(KmsError::UnsupportedDimensions);
        }
        let mut map = MapDumb { handle: create.handle, ..Default::default() };
        if let Err(e) = call(fd, MAP_DUMB, &mut map) {
            let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy); return Err(e);
        }
        let offset = match i64::try_from(map.offset) {
            Ok(v) => v,
            Err(_) => { let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy); return Err(KmsError::UnsupportedDimensions); }
        };
        // SAFETY: MAP_DUMB returned this offset for this live handle on this DRM fd; size is
        // bounded above, and the mapping is retained until munmap before handle destruction.
        let raw = unsafe { mmap(core::ptr::null_mut(), size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset) };
        if raw as isize == -1 {
            let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(last_error());
        }
        let map_ptr = match NonNull::new(raw.cast::<u8>()) {
            Some(p) => p,
            None => { let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy); return Err(KmsError::System(12)); }
        };
        let mut fb = FbCmd { width, height, pitch: create.pitch, bpp: 32, depth: 24, handle: create.handle, ..Default::default() };
        if let Err(e) = call(fd, ADDFB, &mut fb) {
            // SAFETY: map_ptr/size are exactly the successful mmap region above.
            unsafe { munmap(map_ptr.as_ptr().cast(), size); }
            let mut destroy = DestroyDumb { handle: create.handle }; let _ = call(fd, DESTROY_DUMB, &mut destroy); return Err(e);
        }
        Ok(Self { handle: create.handle, fb: fb.fb_id, pitch: create.pitch, size, map: map_ptr })
    }
    fn copy_frame(&mut self, frame: PixelFrame<'_>, height: u32, width: u32) -> Result<(), KmsError> {
        let layout = validate_frame(frame).map_err(KmsError::Present)?;
        if layout.width != width || layout.height != height { return Err(KmsError::UnsupportedDimensions); }
        let row = usize::try_from(width.checked_mul(4).ok_or(KmsError::UnsupportedDimensions)?).map_err(|_| KmsError::UnsupportedDimensions)?;
        let src_stride = usize::try_from(frame.stride).map_err(|_| KmsError::UnsupportedDimensions)?;
        let dst_stride = usize::try_from(self.pitch).map_err(|_| KmsError::UnsupportedDimensions)?;
        let extent = dst_stride.checked_mul(height as usize).ok_or(KmsError::UnsupportedDimensions)?;
        if extent > self.size { return Err(KmsError::UnsupportedDimensions); }
        for y in 0..height as usize {
            // SAFETY: bounds are established by validate_frame and the dumb-buffer extent;
            // each row copy is only `width*4` bytes inside both complete rows.
            unsafe { core::ptr::copy_nonoverlapping(frame.pixels.as_ptr().add(y * src_stride), self.map.as_ptr().add(y * dst_stride), row); }
        }
        Ok(())
    }
}

pub struct KmsDisplay {
    drm: File, tty: File, active_vt: u16, old_kd_mode: c_int, old_crtc: Crtc,
    old_connectors: [u32; MAX_OBJECTS], old_connector_count: u32,
    crtc_id: u32, mode: ModeInfo, connector: u32, width: u32, height: u32,
    buffers: [Option<Buffer>; 2], front: usize, started: bool, restored: bool, faulted: bool,
}

/// Verify caller-selected VT and DRM node, acquire master, and prepare bounded scanout buffers.
/// Opening the path alone never changes the visible mode; `start` is explicit.
pub fn open_kms(drm_path: &str, tty_path: &str) -> Result<KmsDisplay, KmsError> {
    if !valid_card_path(drm_path) || !valid_tty_path(tty_path) { return Err(KmsError::InvalidDevicePath); }
    let tty = OpenOptions::new().read(true).write(true).custom_flags(O_CLOEXEC | O_NOCTTY).open(tty_path).map_err(sys_error)?;
    let mut vt = VtState::default();
    call(tty.as_raw_fd(), VT_GETSTATE, &mut vt)?;
    let expected = tty_number(tty_path).ok_or(KmsError::InvalidDevicePath)?;
    if vt.active != expected { return Err(KmsError::InactiveVt); }
    let mut kd = -1i32;
    call(tty.as_raw_fd(), KDGETMODE, &mut kd)?;
    if kd != KD_TEXT { return Err(KmsError::VtNotText); }
    let mut vt_mode = VtMode::default();
    call(tty.as_raw_fd(), VT_GETMODE, &mut vt_mode)?;
    if vt_mode.mode != VT_AUTO { return Err(KmsError::VtNotText); }

    let drm = OpenOptions::new().read(true).write(true).custom_flags(O_CLOEXEC).open(drm_path).map_err(sys_error)?;
    prepare(drm, tty, vt.active, kd)
}

fn prepare(drm: File, tty: File, active_vt: u16, old_kd_mode: c_int) -> Result<KmsDisplay, KmsError> {
    let fd = drm.as_raw_fd();
    let mut res = CardResources::default(); call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count == 0 || res.crtc_count as usize > MAX_OBJECTS || res.connector_count == 0 || res.connector_count as usize > MAX_OBJECTS || res.encoder_count == 0 || res.encoder_count as usize > MAX_OBJECTS { return Err(KmsError::UnsupportedTopology); }
    let mut crtc_ids = [0u32; MAX_OBJECTS]; let mut conn_ids = [0u32; MAX_OBJECTS]; let mut enc_ids = [0u32; MAX_OBJECTS];
    res.crtc_ptr = crtc_ids.as_mut_ptr() as u64; res.connector_ptr = conn_ids.as_mut_ptr() as u64; res.encoder_ptr = enc_ids.as_mut_ptr() as u64;
    call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count as usize > MAX_OBJECTS || res.connector_count as usize > MAX_OBJECTS || res.encoder_count as usize > MAX_OBJECTS {
        return Err(KmsError::UnsupportedTopology);
    }
    let mut chosen: Option<(u32, u32, ModeInfo)> = None;
    for &cid in conn_ids.iter().take(res.connector_count as usize) {
        let mut c = Connector { connector_id: cid, ..Default::default() }; call(fd, GETCONNECTOR, &mut c)?;
        if c.connection != CONNECTED || c.mode_count == 0 || c.mode_count as usize > MAX_MODES || c.encoder_count == 0 || c.encoder_count as usize > MAX_OBJECTS { continue; }
        let mut modes = [ModeInfo::default(); MAX_MODES]; let mut encoder_ids = [0u32; MAX_OBJECTS];
        c.modes_ptr = modes.as_mut_ptr() as u64; c.encoders_ptr = encoder_ids.as_mut_ptr() as u64;
        call(fd, GETCONNECTOR, &mut c)?;
        if c.mode_count as usize > MAX_MODES || c.encoder_count as usize > MAX_OBJECTS { return Err(KmsError::UnsupportedTopology); }
        let mode = modes.iter().take(c.mode_count as usize).find(|m| m.kind & MODE_PREFERRED != 0).copied()
            .or_else(|| modes.first().copied()).ok_or(KmsError::NoConnectedMode)?;
        if mode.hdisplay == 0 || mode.vdisplay == 0 || mode.hdisplay as u32 > MAX_WIDTH || mode.vdisplay as u32 > MAX_HEIGHT { continue; }
        for &eid in encoder_ids.iter().take(c.encoder_count as usize) {
            let mut e = Encoder { id: eid, ..Default::default() }; call(fd, GETENCODER, &mut e)?;
            let ci = if e.crtc_id != 0 {
                let current = crtc_ids.iter().take(res.crtc_count as usize).position(|id| *id == e.crtc_id)
                    .ok_or(KmsError::UnsupportedTopology)?;
                if e.possible_crtcs & (1u32 << current) == 0 { return Err(KmsError::UnsupportedTopology); }
                Some(current)
            } else {
                (0..res.crtc_count as usize).find(|i| e.possible_crtcs & (1u32 << i) != 0)
            };
            if let Some(index) = ci {
                let selected_crtc = *crtc_ids.get(index).ok_or(KmsError::UnsupportedTopology)?;
                if chosen.is_some_and(|existing| existing.0 != cid) { return Err(KmsError::UnsupportedTopology); }
                chosen = Some((cid, selected_crtc, mode));
                break;
            }
        }
    }
    let (connector, crtc_id, mode) = chosen.ok_or(KmsError::NoConnectedMode)?;
    let mut old_crtc = Crtc { crtc_id, ..Default::default() }; call(fd, GETCRTC, &mut old_crtc)?;
    let mut old_connectors = [0u32; MAX_OBJECTS]; let mut old_connector_count = 0u32;
    // Preserve all currently routed connectors only if there is at most the selected one.
    for &cid in conn_ids.iter().take(res.connector_count as usize) {
        let mut c = Connector { connector_id: cid, ..Default::default() }; call(fd, GETCONNECTOR, &mut c)?;
        if c.encoder_id != 0 {
            let mut e = Encoder { id: c.encoder_id, ..Default::default() }; call(fd, GETENCODER, &mut e)?;
            if e.crtc_id == crtc_id {
                if cid != connector { return Err(KmsError::UnsupportedTopology); }
                let slot = old_connectors.get_mut(old_connector_count as usize).ok_or(KmsError::UnsupportedTopology)?;
                *slot = cid; old_connector_count += 1;
            }
        }
    }
    // Discovery above is read-only and non-master, avoiding GETCONNECTOR's current-master
    // forced-probe behavior. Acquire control only after VT and route checks have passed.
    call_unit(fd, SET_MASTER).map_err(|_| KmsError::DrmMasterUnavailable)?;
    let width = u32::from(mode.hdisplay); let height = u32::from(mode.vdisplay);
    let b0 = Buffer::create(fd, width, height)?;
    let b1 = match Buffer::create(fd, width, height) { Ok(b) => b, Err(e) => { destroy_buffer(fd, b0); return Err(e); } };
    Ok(KmsDisplay { drm, tty, active_vt, old_kd_mode, old_crtc, old_connectors, old_connector_count, crtc_id, mode, connector, width, height, buffers: [Some(b0),Some(b1)], front: 0, started: false, restored: false, faulted: false })
}

impl KmsDisplay {
    pub fn dimensions(&self) -> (u32, u32) { (self.width, self.height) }
    /// Enter graphics mode and set the selected CRTC to a known blank front buffer.
    pub fn start(&mut self) -> Result<(), KmsError> {
        if self.started { return Ok(()); }
        self.ensure_active()?;
        self.set_kd_mode(KD_GRAPHICS)?;
        let front = self.buffers.first().and_then(Option::as_ref).ok_or(KmsError::UnsupportedTopology)?;
        let mut crtc = Crtc { set_connectors_ptr: (&self.connector as *const u32) as u64, connector_count: 1,
            crtc_id: self.crtc_id, fb_id: front.fb, mode_valid: 1, mode: self.mode, ..Default::default() };
        if let Err(e) = call(self.drm.as_raw_fd(), SETCRTC, &mut crtc) {
            let mut old = self.old_crtc;
            old.set_connectors_ptr = self.old_connectors.as_ptr() as u64;
            old.connector_count = self.old_connector_count;
            let crtc_restore = call(self.drm.as_raw_fd(), SETCRTC, &mut old);
            let tty_restore = self.set_kd_mode(self.old_kd_mode);
            // If either rollback operation failed, Drop retries the saved restoration sequence.
            self.started = crtc_restore.is_err() || tty_restore.is_err();
            if let Err(rollback) = crtc_restore.or(tty_restore) {
                return Err(KmsError::CleanupFailed { operation: system_code(e), rollback: system_code(rollback) });
            }
            return Err(e);
        }
        self.started = true; self.front = 0; Ok(())
    }
    /// Copy the whole validated frame into the back buffer and flip at vblank; returns only
    /// after the kernel's DRM_EVENT_FLIP_COMPLETE event has been read.
    pub fn present(&mut self, frame: PixelFrame<'_>) -> Result<(), KmsError> {
        if !self.started { return Err(KmsError::VtNotText); }
        if self.faulted { return Err(KmsError::MalformedEvent); }
        self.ensure_active()?;
        let back = 1 - self.front;
        let buffer = self.buffers.get_mut(back).and_then(Option::as_mut).ok_or(KmsError::UnsupportedTopology)?;
        buffer.copy_frame(frame, self.height, self.width)?;
        let mut flip = PageFlip { crtc_id: self.crtc_id, fb_id: buffer.fb, flags: FLIP_EVENT, ..Default::default() };
        call(self.drm.as_raw_fd(), PAGE_FLIP, &mut flip)?;
        if let Err(e) = wait_flip(self.drm.as_raw_fd()) { self.faulted = true; return Err(e); }
        self.front = back; Ok(())
    }
    fn ensure_active(&self) -> Result<(), KmsError> {
        let mut vt = VtState::default(); call(self.tty.as_raw_fd(), VT_GETSTATE, &mut vt)?;
        if vt.active != self.active_vt { return Err(KmsError::InactiveVt); } Ok(())
    }
    fn set_kd_mode(&self, mode: c_int) -> Result<(), KmsError> {
        // SAFETY: KDSETMODE is an argument-by-value ioctl (`KD_TEXT` or `KD_GRAPHICS`).
        let rc = unsafe { ioctl(self.tty.as_raw_fd(), KDSETMODE, mode as c_ulong) };
        if rc < 0 { Err(last_error()) } else { Ok(()) }
    }
    /// Restore the saved CRTC and VT mode before destroying buffers and dropping DRM master.
    pub fn close(mut self) -> Result<(), KmsError> { self.cleanup() }
    fn cleanup(&mut self) -> Result<(), KmsError> {
        let fd = self.drm.as_raw_fd(); let mut failure = None;
        let mut may_remove_fbs = !self.started;
        if self.started && !self.restored {
            let active = self.ensure_active().is_ok();
            if active {
                let mut old = self.old_crtc;
                old.set_connectors_ptr = self.old_connectors.as_ptr() as u64;
                old.connector_count = self.old_connector_count;
                match call(fd, SETCRTC, &mut old) { Ok(()) => may_remove_fbs = true, Err(e) => failure = Some(e) }
                if let Err(e) = self.set_kd_mode(self.old_kd_mode) { if failure.is_none() { failure = Some(e); } }
            } else { failure = Some(KmsError::InactiveVt); }
            self.restored = true;
        }
        for slot in &mut self.buffers {
            if let Some(b) = slot.take() {
                let result = if may_remove_fbs { destroy_buffer(fd, b) } else { unmap_buffer(&b) };
                if let Err(e) = result { if failure.is_none() { failure = Some(e); } }
            }
        }
        if may_remove_fbs {
            if let Err(e) = ioctl_call_unit(fd, DROP_MASTER) { if failure.is_none() { failure = Some(e); } }
        }
        if let Some(e) = failure { Err(e) } else { Ok(()) }
    }
}
impl Drop for KmsDisplay { fn drop(&mut self) { let _ = self.cleanup(); } }

fn destroy_buffer(fd: c_int, b: Buffer) -> Result<(), KmsError> {
    let mut failure = unmap_buffer(&b).err();
    let mut fb = b.fb; if let Err(e) = ioctl_arg(fd, RMFB, &mut fb) { if failure.is_none() { failure = Some(e); } }
    let mut d = DestroyDumb { handle: b.handle }; if let Err(e) = call(fd, DESTROY_DUMB, &mut d) { if failure.is_none() { failure = Some(e); } }
    match failure { Some(e) => Err(e), None => Ok(()) }
}
fn unmap_buffer(b: &Buffer) -> Result<(), KmsError> {
    // SAFETY: mapping pointer/length are the successful mmap region owned by this Buffer.
    let rc = unsafe { munmap(b.map.as_ptr().cast(), b.size) };
    if rc < 0 { Err(last_error()) } else { Ok(()) }
}
fn wait_flip(fd: c_int) -> Result<(), KmsError> {
    let mut fds = PollFd { fd, events: POLLIN, revents: 0 };
    // SAFETY: fds points to one initialized pollfd for the duration of poll.
    let n = unsafe { poll(&mut fds, 1, WAIT_MS) };
    if n == 0 { return Err(KmsError::Timeout); } if n < 0 { return Err(last_error()); }
    if fds.revents & POLLIN == 0 { return Err(KmsError::MalformedEvent); }
    let mut event = [0u8; 256];
    // SAFETY: event is a writable 256-byte stack buffer and read is bounded to its length.
    let got = unsafe { read(fd, event.as_mut_ptr().cast(), event.len()) };
    if got < 0 { return Err(last_error()); }
    let got = got as usize;
    if got < 8 { return Err(KmsError::MalformedEvent); }
    let kind = u32::from_ne_bytes(event.get(0..4).ok_or(KmsError::MalformedEvent)?.try_into().map_err(|_| KmsError::MalformedEvent)?);
    let len = u32::from_ne_bytes(event.get(4..8).ok_or(KmsError::MalformedEvent)?.try_into().map_err(|_| KmsError::MalformedEvent)?) as usize;
    if kind != EVENT_FLIP_COMPLETE || len < 8 || len > got { return Err(KmsError::MalformedEvent); }
    Ok(())
}

// Safe wrapper: each call's pointer is to a live repr(C) UAPI structure of the request's exact size.
fn call<T>(fd: c_int, request: c_ulong, value: &mut T) -> Result<(), KmsError> {
    // SAFETY: callers pass the matching repr(C) UAPI record, initialized and alive through ioctl.
    let rc = unsafe { ioctl(fd, request, value as *mut T) };
    if rc < 0 { Err(last_error()) } else { Ok(()) }
}
fn call_unit(fd: c_int, request: c_ulong) -> Result<(), KmsError> { ioctl_call_unit(fd, request) }
fn ioctl_call_unit(fd: c_int, request: c_ulong) -> Result<(), KmsError> {
    // SAFETY: request is DRM_IOCTL_SET_MASTER or DROP_MASTER, both argument-free UAPI ioctls.
    let rc = unsafe { ioctl(fd, request) }; if rc < 0 { Err(last_error()) } else { Ok(()) }
}
fn ioctl_arg(fd: c_int, request: c_ulong, value: &mut u32) -> Result<(), KmsError> {
    // SAFETY: RMFB takes a pointer to one unsigned framebuffer ID.
    let rc = unsafe { ioctl(fd, request, value as *mut u32) }; if rc < 0 { Err(last_error()) } else { Ok(()) }
}
fn last_error() -> KmsError { KmsError::System(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)) }
fn system_code(error: KmsError) -> i32 { match error { KmsError::System(code) => code, _ => -1 } }
fn sys_error(e: std::io::Error) -> KmsError { KmsError::System(e.raw_os_error().unwrap_or(-1)) }
fn valid_card_path(s: &str) -> bool { s.strip_prefix("/dev/dri/card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) }
fn valid_tty_path(s: &str) -> bool { tty_number(s).is_some() }
fn tty_number(s: &str) -> Option<u16> { let n = s.strip_prefix("/dev/tty")?; let x = n.parse::<u16>().ok()?; (x > 0 && x < 16).then_some(x) }

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn uapi_layouts_match_linux_64_bit_abi() {
        assert_eq!(size_of::<ModeInfo>(), 68); assert_eq!(size_of::<CardResources>(), 64);
        assert_eq!(size_of::<Crtc>(), 104); assert_eq!(size_of::<Connector>(), 80);
        assert_eq!(size_of::<CreateDumb>(), 32); assert_eq!(size_of::<PageFlip>(), 24);
        assert_eq!(GETRESOURCES, 0xC04064A0); assert_eq!(GETCONNECTOR, 0xC05064A7);
        assert_eq!(EVIOCGVERSION, 0x80044501);
    }
    #[test] fn explicit_device_paths_only() {
        assert!(valid_card_path("/dev/dri/card0")); assert!(!valid_card_path("/dev/dri/renderD128"));
        assert!(valid_tty_path("/dev/tty7")); assert!(!valid_tty_path("/dev/tty17")); assert!(!valid_tty_path("/dev/ttyS0"));
    }
}

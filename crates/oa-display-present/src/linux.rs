//! Linux KMS and VT shell. Every raw OS operation is isolated here.
//!
//! ABI layouts and ioctl numbers follow Linux UAPI headers `drm.h`, `drm_mode.h`,
//! `drm_fourcc.h`, `vt.h`, and `kd.h`. Only legacy single-plane linear XRGB8888
//! dumb buffers are accepted. No atomic properties, modifiers, leases, or desktop stack.

#![allow(unsafe_code)] // RULESET A0: sole OS syscall/mmap module in the display-presentation crate.

use crate::{
    decode_evdev_event, kms_cleanup_may_destroy, kms_transition, validate_frame, InputError,
    InputEvent, InputRecord, InputRecordKind, InputSnapshot, KmsLifecycle, KmsTransition,
    PixelFrame, PresentError, EV_SYN, MAX_FRAME_BYTES, MAX_HEIGHT, MAX_WIDTH, SYN_DROPPED,
};
use core::ffi::{c_int, c_ulong, c_void};
use core::ptr::NonNull;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::rc::Rc;
use std::time::{Duration, Instant};

const DRM_IOCTL_BASE: u32 = b'd' as u32;
const IOC_READ: u32 = 2;
const IOC_WRITE: u32 = 1;
const fn ioc_for(dir: u32, kind: u32, nr: u32, size: usize) -> c_ulong {
    ((dir << 30) | (kind << 8) | nr | ((size as u32) << 16)) as c_ulong
}
const fn ioc(dir: u32, nr: u32, size: usize) -> c_ulong {
    ioc_for(dir, DRM_IOCTL_BASE, nr, size)
}
const fn drm_iowr(nr: u32, size: usize) -> c_ulong {
    ioc(IOC_READ | IOC_WRITE, nr, size)
}
const fn drm_io(nr: u32) -> c_ulong {
    ioc(0, nr, 0)
}

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
const VT_SETMODE: c_ulong = 0x5602;
const VT_GETSTATE: c_ulong = 0x5603;
const VT_RELDISP: c_ulong = 0x5605;
const KDGETMODE: c_ulong = 0x4B3B;
const KDSETMODE: c_ulong = 0x4B3A;
const KD_TEXT: c_int = 0;
const KD_GRAPHICS: c_int = 1;
const VT_AUTO: u8 = 0;
const VT_PROCESS: u8 = 1;
const VT_RELEASE_OK: c_ulong = 1;
const SIG_BLOCK: c_int = 0;
const SIG_SETMASK: c_int = 2;
const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;
#[cfg(test)]
const SI_USER: i32 = 0;
const SI_KERNEL: i32 = 0x80;
const SFD_NONBLOCK: c_int = 0x800;
const SFD_CLOEXEC: c_int = O_CLOEXEC;
const CONNECTED: u32 = 1;
const MODE_PREFERRED: u32 = 1 << 3;
const FLIP_EVENT: u32 = 1;
const EVENT_FLIP_COMPLETE: u32 = 2;
const FLIP_EVENT_MIN_BYTES: usize = 32;
const MAX_OBJECTS: usize = 32;
const MAX_MODES: usize = 64;
const WAIT_MS: c_int = 1000;
const MAX_INPUT_WAIT_MS: u32 = 1000;
const MAX_INTERRUPTS: u8 = 8;
const MAX_RESYNC_RECORDS: u32 = 1024;
const SIGNAL_INFO_BYTES: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}
#[repr(C)]
#[derive(Default)]
struct CardResources {
    fb_ptr: u64,
    crtc_ptr: u64,
    connector_ptr: u64,
    encoder_ptr: u64,
    fb_count: u32,
    crtc_count: u32,
    connector_count: u32,
    encoder_count: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Crtc {
    connectors_ptr: u64,
    connector_count: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}
#[repr(C)]
#[derive(Default)]
struct Encoder {
    id: u32,
    kind: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    clones: u32,
}
#[repr(C)]
#[derive(Default)]
struct Connector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    mode_count: u32,
    prop_count: u32,
    encoder_count: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}
#[repr(C)]
#[derive(Default)]
struct FbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}
#[repr(C)]
#[derive(Default)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}
#[repr(C)]
#[derive(Default)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}
#[repr(C)]
#[derive(Default)]
struct DestroyDumb {
    handle: u32,
}
#[repr(C)]
#[derive(Default)]
struct PageFlip {
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    reserved: u32,
    user_data: u64,
}
#[repr(C)]
#[derive(Default)]
struct VtState {
    active: u16,
    signal: u16,
    state: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VtMode {
    mode: u8,
    waitv: u8,
    relsig: i16,
    acqsig: i16,
    frsig: i16,
}
#[repr(C)]
#[derive(Default)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SigSet {
    words: [u64; 16],
}

const _: () = assert!(size_of::<ModeInfo>() == 68);
const _: () = assert!(size_of::<CardResources>() == 64);
const _: () = assert!(size_of::<Crtc>() == 104);
const _: () = assert!(size_of::<Connector>() == 80);
const _: () = assert!(size_of::<CreateDumb>() == 32);
const _: () = assert!(size_of::<PageFlip>() == 24);

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn poll(fds: *mut PollFd, count: usize, timeout: c_int) -> c_int;
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
    fn signalfd(fd: c_int, mask: *const SigSet, flags: c_int) -> c_int;
    fn sigemptyset(set: *mut SigSet) -> c_int;
    fn sigaddset(set: *mut SigSet, signal: c_int) -> c_int;
    fn sigismember(set: *const SigSet, signal: c_int) -> c_int;
    fn pthread_sigmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
    fn getpid() -> c_int;
    fn gettid() -> c_int;
}
const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const POLLIN: i16 = 1;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 16;
const POLLNVAL: i16 = 32;
const O_NONBLOCK: i32 = 0x800;
const O_CLOEXEC: i32 = 0x80000;
const O_NOCTTY: i32 = 0x100;
const EVIOCGVERSION: c_ulong = ioc_for(IOC_READ, b'E' as u32, 0x01, 4);
const EVIOCGKEY: c_ulong = ioc_for(IOC_READ, b'E' as u32, 0x18, 96);
const EV_ABS: u32 = 3;
const ABS_AXIS_COUNT: usize = 64;
const EVIOCGBIT_ABS: c_ulong = ioc_for(IOC_READ, b'E' as u32, 0x20 + EV_ABS, 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}
const _: () = assert!(size_of::<AbsInfo>() == 24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmsError {
    Present(PresentError),
    InvalidDevicePath,
    InactiveVt,
    VtNotText,
    DrmMasterUnavailable,
    NoConnectedMode,
    UnsupportedTopology,
    UnsupportedDimensions,
    Timeout,
    MalformedEvent,
    DedicatedOwnerProcessRequired,
    SignalControl(i32),
    UnexpectedSignal(i32),
    OwnerMustTerminate,
    InvalidLifecycle,
    RestoreFailed,
    ReleaseFailed(i32),
    Faulted,
    System(i32),
    CleanupFailed { operation: i32, rollback: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputDeviceError {
    InvalidPath,
    Disconnected,
    TruncatedRecord,
    WaitLimit,
    RecordLimit,
    Timeout,
    Decode(InputError),
    System(i32),
}

/// Bounded Linux evdev reader for one explicitly selected `/dev/input/eventN` node.
pub struct EvdevInput {
    file: File,
    record: [u8; 24],
    filled: usize,
    desynchronized: bool,
    discarded: u32,
}
pub fn open_evdev(path: &str) -> Result<EvdevInput, InputDeviceError> {
    let number = path
        .strip_prefix("/dev/input/event")
        .ok_or(InputDeviceError::InvalidPath)?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InputDeviceError::InvalidPath);
    }
    // seal-allow: open only the caller-selected validated evdev device node
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK | O_CLOEXEC)
        .open(path)
        .map_err(|e| InputDeviceError::System(e.raw_os_error().unwrap_or(-1)))?;
    let mut version = 0i32;
    // SAFETY: EVIOCGVERSION writes one initialized 32-bit version value to this live pointer.
    let rc = unsafe { ioctl(file.as_raw_fd(), EVIOCGVERSION, &mut version as *mut i32) };
    if rc < 0 {
        return Err(InputDeviceError::System(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        ));
    }
    Ok(EvdevInput {
        file,
        record: [0; 24],
        filled: 0,
        desynchronized: false,
        discarded: 0,
    })
}
impl EvdevInput {
    /// Read one complete input record within `timeout_ms` (capped at one second).
    /// SYN_DROPPED invalidates actions until `resynchronize` drains queued input and snapshots state.
    pub fn read_record(&mut self, timeout_ms: u32) -> Result<InputRecord, InputDeviceError> {
        if timeout_ms > MAX_INPUT_WAIT_MS {
            return Err(InputDeviceError::WaitLimit);
        }
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let deadline = Instant::now() + timeout;
        let mut interruptions = 0u8;
        while self.filled < self.record.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = remaining.as_millis().min(i32::MAX as u128) as c_int;
            let mut fds = PollFd {
                fd: self.file.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            };
            // SAFETY: fds points to one initialized pollfd for the duration of poll.
            let ready = unsafe { poll(&mut fds, 1, millis) };
            if ready == 0 {
                return Err(InputDeviceError::Timeout);
            }
            if ready < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions <= MAX_INTERRUPTS {
                        continue;
                    }
                }
                return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1)));
            }
            if fds.revents & POLLIN == 0 {
                return Err(InputDeviceError::Disconnected);
            }
            let target = self
                .record
                .get_mut(self.filled..)
                .ok_or(InputDeviceError::TruncatedRecord)?;
            match self.file.read(target) {
                Ok(0) if self.filled == 0 => return Err(InputDeviceError::Disconnected),
                Ok(0) => {
                    self.filled = 0;
                    return Err(InputDeviceError::TruncatedRecord);
                }
                Ok(n) => self.filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions <= MAX_INTERRUPTS {
                        continue;
                    }
                    return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1)));
                }
                Err(e) => return Err(InputDeviceError::System(e.raw_os_error().unwrap_or(-1))),
            }
        }
        let decoded = decode_evdev_event(&self.record);
        self.filled = 0;
        match decoded.map_err(InputDeviceError::Decode)? {
            Some(event) if event.kind == EV_SYN && event.code == SYN_DROPPED => {
                self.desynchronized = true;
                self.discarded = 0;
                Ok(input_record(
                    InputRecordKind::SyncLost,
                    InputEvent::default(),
                    crate::input_snapshot_new(),
                ))
            }
            Some(_) if self.desynchronized => {
                self.discarded = self.discarded.saturating_add(1);
                Ok(input_record(
                    InputRecordKind::Discarded,
                    InputEvent::default(),
                    InputSnapshot {
                        discarded_records: self.discarded,
                        ..crate::input_snapshot_new()
                    },
                ))
            }
            Some(event) => Ok(input_record(
                InputRecordKind::Event,
                event,
                crate::input_snapshot_new(),
            )),
            None if self.desynchronized => {
                self.discarded = self.discarded.saturating_add(1);
                Ok(input_record(
                    InputRecordKind::Discarded,
                    InputEvent::default(),
                    InputSnapshot {
                        discarded_records: self.discarded,
                        ..crate::input_snapshot_new()
                    },
                ))
            }
            None => Ok(input_record(
                InputRecordKind::SyncReport,
                InputEvent::default(),
                crate::input_snapshot_new(),
            )),
        }
    }

    /// Drain queued records before querying current key and absolute-axis state.
    /// Actions remain invalid on every refusal or error; held keys are reported as state only.
    /// A zero timeout means nonblocking bounded drain; positive values cap total elapsed time.
    pub fn resynchronize(
        &mut self,
        max_records: u32,
        timeout_ms: u32,
    ) -> Result<InputSnapshot, InputDeviceError> {
        self.desynchronized = true;
        if max_records == 0 || max_records > MAX_RESYNC_RECORDS {
            return Err(InputDeviceError::RecordLimit);
        }
        if timeout_ms > MAX_INPUT_WAIT_MS {
            return Err(InputDeviceError::WaitLimit);
        }
        if self.filled != 0 {
            self.filled = 0;
            return Err(InputDeviceError::TruncatedRecord);
        }

        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let started = Instant::now();
        let mut records = 0u32;
        let mut interruptions = 0u8;
        let mut wire = [0u8; 24];
        loop {
            if timeout_ms != 0 && started.elapsed() > timeout {
                return Err(InputDeviceError::Timeout);
            }
            if records == max_records {
                return Err(InputDeviceError::RecordLimit);
            }
            match self.file.read(&mut wire) {
                Ok(0) => return Err(InputDeviceError::Disconnected),
                Ok(n) if n != wire.len() => return Err(InputDeviceError::TruncatedRecord),
                Ok(_) => {
                    interruptions = 0;
                    records += 1;
                    self.discarded = self.discarded.saturating_add(1);
                    decode_evdev_event(&wire).map_err(InputDeviceError::Decode)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if timeout_ms != 0 && started.elapsed() > timeout {
                        return Err(InputDeviceError::Timeout);
                    }
                    let snapshot = query_input_snapshot(self.file.as_raw_fd(), self.discarded)?;
                    if timeout_ms != 0 && started.elapsed() > timeout {
                        return Err(InputDeviceError::Timeout);
                    }
                    self.desynchronized = false;
                    return Ok(snapshot);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions > MAX_INTERRUPTS {
                        return Err(InputDeviceError::System(error.raw_os_error().unwrap_or(-1)));
                    }
                }
                Err(error) => {
                    return Err(InputDeviceError::System(error.raw_os_error().unwrap_or(-1)))
                }
            }
        }
    }
}
fn input_record(kind: InputRecordKind, event: InputEvent, snapshot: InputSnapshot) -> InputRecord {
    InputRecord {
        kind,
        reserved: [0; 7],
        event,
        snapshot,
    }
}
fn query_input_snapshot(fd: c_int, discarded: u32) -> Result<InputSnapshot, InputDeviceError> {
    let mut snapshot = InputSnapshot {
        discarded_records: discarded,
        relative_motion_lost: 1,
        ..crate::input_snapshot_new()
    };
    // SAFETY: EVIOCGKEY writes exactly this initialized 96-byte key bitmap.
    let rc = unsafe { ioctl(fd, EVIOCGKEY, snapshot.key_bits.as_mut_ptr()) };
    if rc < 0 {
        return Err(InputDeviceError::System(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        ));
    }
    // SAFETY: EVIOCGBIT(EV_ABS) writes exactly eight bytes into the ABS capability bitmap.
    let rc = unsafe { ioctl(fd, EVIOCGBIT_ABS, snapshot.absolute_axis_bits.as_mut_ptr()) };
    if rc < 0 {
        return Err(InputDeviceError::System(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
        ));
    }
    for axis in 0..ABS_AXIS_COUNT {
        let byte = snapshot
            .absolute_axis_bits
            .get(axis / 8)
            .copied()
            .ok_or(InputDeviceError::Decode(InputError::Truncated))?;
        if byte & (1u8 << (axis % 8)) != 0 {
            let request = ioc_for(
                IOC_READ,
                b'E' as u32,
                0x40 + axis as u32,
                size_of::<AbsInfo>(),
            );
            let mut info = AbsInfo::default();
            // SAFETY: EVIOCGABS(axis) writes one 24-byte input_absinfo into this live struct.
            let rc = unsafe { ioctl(fd, request, &mut info as *mut AbsInfo) };
            if rc < 0 {
                return Err(InputDeviceError::System(
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
                ));
            }
            let value = snapshot
                .absolute_axis_values
                .get_mut(axis)
                .ok_or(InputDeviceError::Decode(InputError::Truncated))?;
            *value = info.value;
        }
    }
    Ok(snapshot)
}

struct Buffer {
    handle: u32,
    fb: u32,
    pitch: u32,
    size: usize,
    map: NonNull<u8>,
}
impl Buffer {
    fn create(fd: c_int, width: u32, height: u32) -> Result<Self, KmsError> {
        let mut create = CreateDumb {
            width,
            height,
            bpp: 32,
            ..Default::default()
        };
        call(fd, CREATE_DUMB, &mut create)?;
        let size = usize::try_from(create.size).map_err(|_| KmsError::UnsupportedDimensions)?;
        if size == 0
            || size > MAX_FRAME_BYTES
            || create.pitch
                < width
                    .checked_mul(4)
                    .ok_or(KmsError::UnsupportedDimensions)?
        {
            let mut destroy = DestroyDumb {
                handle: create.handle,
            };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(KmsError::UnsupportedDimensions);
        }
        let mapped_extent = match usize::try_from(create.pitch)
            .ok()
            .and_then(|p| usize::try_from(height).ok().and_then(|h| p.checked_mul(h)))
        {
            Some(extent) => extent,
            None => {
                let mut destroy = DestroyDumb {
                    handle: create.handle,
                };
                let _ = call(fd, DESTROY_DUMB, &mut destroy);
                return Err(KmsError::UnsupportedDimensions);
            }
        };
        if mapped_extent > size {
            let mut destroy = DestroyDumb {
                handle: create.handle,
            };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(KmsError::UnsupportedDimensions);
        }
        let mut map = MapDumb {
            handle: create.handle,
            ..Default::default()
        };
        if let Err(e) = call(fd, MAP_DUMB, &mut map) {
            let mut destroy = DestroyDumb {
                handle: create.handle,
            };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(e);
        }
        let offset = match i64::try_from(map.offset) {
            Ok(v) => v,
            Err(_) => {
                let mut destroy = DestroyDumb {
                    handle: create.handle,
                };
                let _ = call(fd, DESTROY_DUMB, &mut destroy);
                return Err(KmsError::UnsupportedDimensions);
            }
        };
        // SAFETY: MAP_DUMB returned this offset for this live handle on this DRM fd; size is
        // bounded above, and the mapping is retained until munmap before handle destruction.
        let raw = unsafe {
            mmap(
                core::ptr::null_mut(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                offset,
            )
        };
        if raw as isize == -1 {
            let mut destroy = DestroyDumb {
                handle: create.handle,
            };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(last_error());
        }
        let map_ptr = match NonNull::new(raw.cast::<u8>()) {
            Some(p) => p,
            None => {
                // SAFETY: mmap succeeded at address zero; remove this exact mapping before
                // destroying its DRM handle even though Rust cannot represent it as NonNull.
                let _ = unsafe { munmap(raw, size) };
                let mut destroy = DestroyDumb {
                    handle: create.handle,
                };
                let _ = call(fd, DESTROY_DUMB, &mut destroy);
                return Err(KmsError::System(12));
            }
        };
        // SAFETY: `size` is the bounded successful mapping extent owned by map_ptr.
        // Zeroing ensures the first scanout is deterministic instead of relying on driver state.
        unsafe {
            core::ptr::write_bytes(map_ptr.as_ptr(), 0, size);
        }
        let mut fb = FbCmd {
            width,
            height,
            pitch: create.pitch,
            bpp: 32,
            depth: 24,
            handle: create.handle,
            ..Default::default()
        };
        if let Err(e) = call(fd, ADDFB, &mut fb) {
            // SAFETY: map_ptr/size are exactly the successful mmap region above.
            unsafe {
                munmap(map_ptr.as_ptr().cast(), size);
            }
            let mut destroy = DestroyDumb {
                handle: create.handle,
            };
            let _ = call(fd, DESTROY_DUMB, &mut destroy);
            return Err(e);
        }
        Ok(Self {
            handle: create.handle,
            fb: fb.fb_id,
            pitch: create.pitch,
            size,
            map: map_ptr,
        })
    }
    fn copy_frame(
        &mut self,
        frame: PixelFrame<'_>,
        height: u32,
        width: u32,
    ) -> Result<(), KmsError> {
        let layout = validate_frame(frame).map_err(KmsError::Present)?;
        if layout.width != width || layout.height != height {
            return Err(KmsError::UnsupportedDimensions);
        }
        let row = usize::try_from(
            width
                .checked_mul(4)
                .ok_or(KmsError::UnsupportedDimensions)?,
        )
        .map_err(|_| KmsError::UnsupportedDimensions)?;
        let src_stride =
            usize::try_from(frame.stride).map_err(|_| KmsError::UnsupportedDimensions)?;
        let dst_stride =
            usize::try_from(self.pitch).map_err(|_| KmsError::UnsupportedDimensions)?;
        let extent = dst_stride
            .checked_mul(height as usize)
            .ok_or(KmsError::UnsupportedDimensions)?;
        if extent > self.size {
            return Err(KmsError::UnsupportedDimensions);
        }
        for y in 0..height as usize {
            // SAFETY: bounds are established by validate_frame and the dumb-buffer extent;
            // each row copy is only `width*4` bytes inside both complete rows.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    frame.pixels.as_ptr().add(y * src_stride),
                    self.map.as_ptr().add(y * dst_stride),
                    row,
                );
            }
        }
        Ok(())
    }
}

pub struct KmsDisplay {
    drm: File,
    tty: File,
    active_vt: u16,
    old_kd_mode: c_int,
    old_crtc: Crtc,
    old_connectors: [u32; MAX_OBJECTS],
    old_connector_count: u32,
    crtc_id: u32,
    mode: ModeInfo,
    connector: u32,
    width: u32,
    height: u32,
    buffers: [Option<Buffer>; 2],
    front: usize,
    state: KmsLifecycle,
    flip_pending: bool,
    pending_front: usize,
    release_requested: bool,
    crtc_restored: bool,
    old_vt_mode: VtMode,
    old_signal_mask: SigSet,
    signals: Option<File>,
    owner_tid: c_int,
    _not_send_sync: PhantomData<Rc<()>>,
}

/// Verify caller-selected VT and DRM node, acquire master, and prepare bounded scanout buffers.
/// Opening the path alone never changes the visible mode; `start` is explicit.
pub fn open_kms(drm_path: &str, tty_path: &str, connector_id: u32) -> Result<KmsDisplay, KmsError> {
    if !valid_card_path(drm_path) || !valid_tty_path(tty_path) {
        return Err(KmsError::InvalidDevicePath);
    }
    if !is_dedicated_owner_process() {
        return Err(KmsError::DedicatedOwnerProcessRequired);
    }
    // seal-allow: open only the validated active Linux VT device
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_CLOEXEC | O_NOCTTY)
        .open(tty_path)
        .map_err(sys_error)?;
    let mut vt = VtState::default();
    call(tty.as_raw_fd(), VT_GETSTATE, &mut vt)?;
    let expected = tty_number(tty_path).ok_or(KmsError::InvalidDevicePath)?;
    if vt.active != expected {
        return Err(KmsError::InactiveVt);
    }
    let mut kd = -1i32;
    call(tty.as_raw_fd(), KDGETMODE, &mut kd)?;
    if kd != KD_TEXT {
        return Err(KmsError::VtNotText);
    }
    let mut vt_mode = VtMode::default();
    call(tty.as_raw_fd(), VT_GETMODE, &mut vt_mode)?;
    if vt_mode.mode != VT_AUTO {
        return Err(KmsError::VtNotText);
    }

    // seal-allow: open only the caller-selected validated DRM card device
    let drm = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_CLOEXEC)
        .open(drm_path)
        .map_err(sys_error)?;
    prepare(drm, tty, vt.active, kd, vt_mode, connector_id)
}

fn prepare(
    drm: File,
    tty: File,
    active_vt: u16,
    old_kd_mode: c_int,
    old_vt_mode: VtMode,
    requested_connector: u32,
) -> Result<KmsDisplay, KmsError> {
    let fd = drm.as_raw_fd();
    let mut res = CardResources::default();
    call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count == 0
        || res.crtc_count as usize > MAX_OBJECTS
        || res.connector_count == 0
        || res.connector_count as usize > MAX_OBJECTS
        || res.encoder_count == 0
        || res.encoder_count as usize > MAX_OBJECTS
    {
        return Err(KmsError::UnsupportedTopology);
    }
    let mut crtc_ids = [0u32; MAX_OBJECTS];
    let mut conn_ids = [0u32; MAX_OBJECTS];
    let mut enc_ids = [0u32; MAX_OBJECTS];
    res.crtc_ptr = crtc_ids.as_mut_ptr() as u64;
    res.connector_ptr = conn_ids.as_mut_ptr() as u64;
    res.encoder_ptr = enc_ids.as_mut_ptr() as u64;
    call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count as usize > MAX_OBJECTS
        || res.connector_count as usize > MAX_OBJECTS
        || res.encoder_count as usize > MAX_OBJECTS
    {
        return Err(KmsError::UnsupportedTopology);
    }
    let mut chosen: Option<(u32, u32, ModeInfo)> = None;
    for &cid in conn_ids.iter().take(res.connector_count as usize) {
        if cid != requested_connector {
            continue;
        }
        let mut c = Connector {
            connector_id: cid,
            ..Default::default()
        };
        call(fd, GETCONNECTOR, &mut c)?;
        if c.connection != CONNECTED
            || c.mode_count == 0
            || c.mode_count as usize > MAX_MODES
            || c.encoder_count == 0
            || c.encoder_count as usize > MAX_OBJECTS
        {
            continue;
        }
        let mut modes = [ModeInfo::default(); MAX_MODES];
        let mut encoder_ids = [0u32; MAX_OBJECTS];
        c.modes_ptr = modes.as_mut_ptr() as u64;
        c.encoders_ptr = encoder_ids.as_mut_ptr() as u64;
        call(fd, GETCONNECTOR, &mut c)?;
        if c.mode_count as usize > MAX_MODES || c.encoder_count as usize > MAX_OBJECTS {
            return Err(KmsError::UnsupportedTopology);
        }
        let mode = modes
            .iter()
            .take(c.mode_count as usize)
            .find(|m| m.kind & MODE_PREFERRED != 0)
            .copied()
            .or_else(|| modes.first().copied())
            .ok_or(KmsError::NoConnectedMode)?;
        if mode.hdisplay == 0
            || mode.vdisplay == 0
            || mode.hdisplay as u32 > MAX_WIDTH
            || mode.vdisplay as u32 > MAX_HEIGHT
        {
            continue;
        }
        for &eid in encoder_ids.iter().take(c.encoder_count as usize) {
            let mut e = Encoder {
                id: eid,
                ..Default::default()
            };
            call(fd, GETENCODER, &mut e)?;
            let ci = if e.crtc_id != 0 {
                let current = crtc_ids
                    .iter()
                    .take(res.crtc_count as usize)
                    .position(|id| *id == e.crtc_id)
                    .ok_or(KmsError::UnsupportedTopology)?;
                if e.possible_crtcs & (1u32 << current) == 0 {
                    return Err(KmsError::UnsupportedTopology);
                }
                Some(current)
            } else {
                (0..res.crtc_count as usize).find(|i| e.possible_crtcs & (1u32 << i) != 0)
            };
            if let Some(index) = ci {
                let selected_crtc = *crtc_ids.get(index).ok_or(KmsError::UnsupportedTopology)?;
                if chosen.is_some_and(|existing| existing.0 != cid) {
                    return Err(KmsError::UnsupportedTopology);
                }
                chosen = Some((cid, selected_crtc, mode));
                break;
            }
        }
    }
    let (connector, crtc_id, mode) = chosen.ok_or(KmsError::NoConnectedMode)?;
    // Initial discovery is read-only and non-master. Acquire control only after active-VT and
    // candidate-route checks, then discard the snapshot and recapture topology under master.
    if active_vt != query_active_vt(tty.as_raw_fd())? {
        return Err(KmsError::InactiveVt);
    }
    let mut current_kd = -1i32;
    call(tty.as_raw_fd(), KDGETMODE, &mut current_kd)?;
    let mut current_vt_mode = VtMode::default();
    call(tty.as_raw_fd(), VT_GETMODE, &mut current_vt_mode)?;
    if current_kd != KD_TEXT || current_vt_mode.mode != VT_AUTO {
        return Err(KmsError::VtNotText);
    }
    call_unit(fd, SET_MASTER).map_err(|_| KmsError::DrmMasterUnavailable)?;
    if active_vt != query_active_vt(tty.as_raw_fd())? {
        return Err(KmsError::InactiveVt);
    }
    let (old_crtc, old_connectors, old_connector_count) =
        capture_baseline(fd, connector, crtc_id, mode)?;
    let width = u32::from(mode.hdisplay);
    let height = u32::from(mode.vdisplay);
    let b0 = Buffer::create(fd, width, height)?;
    let b1 = match Buffer::create(fd, width, height) {
        Ok(b) => b,
        Err(e) => {
            let _ = destroy_buffer(fd, b0);
            return Err(e);
        }
    };
    match query_active_vt(tty.as_raw_fd()) {
        Ok(vt) if vt == active_vt => {}
        Ok(_) => {
            let _ = destroy_buffer(fd, b0);
            let _ = destroy_buffer(fd, b1);
            return Err(KmsError::InactiveVt);
        }
        Err(e) => {
            let _ = destroy_buffer(fd, b0);
            let _ = destroy_buffer(fd, b1);
            return Err(e);
        }
    }
    let (signals, old_signal_mask) = match install_vt_process(tty.as_raw_fd()) {
        Ok(v) => v,
        Err(e) => {
            let _ = destroy_buffer(fd, b0);
            let _ = destroy_buffer(fd, b1);
            return Err(e);
        }
    };
    let active_after_setup = query_active_vt(tty.as_raw_fd());
    let setup_still_active = matches!(active_after_setup, Ok(vt) if vt == active_vt);
    if !setup_still_active {
        let mut restore_mode = old_vt_mode;
        let restore = call(tty.as_raw_fd(), VT_SETMODE, &mut restore_mode);
        let _ = destroy_buffer(fd, b0);
        let _ = destroy_buffer(fd, b1);
        if restore.is_err() {
            // VT_PROCESS still names this process. Preserve its signal fd and blocked mask;
            // terminating this owner lets the kernel reset VT control safely.
            core::mem::forget(signals);
            return Err(KmsError::OwnerMustTerminate);
        }
        drop(signals);
        restore_signal_mask(&old_signal_mask)?;
        return match active_after_setup {
            Err(e) => Err(e),
            Ok(_) => Err(KmsError::InactiveVt),
        };
    }
    Ok(KmsDisplay {
        drm,
        tty,
        active_vt,
        old_kd_mode,
        old_crtc,
        old_connectors,
        old_connector_count,
        crtc_id,
        mode,
        connector,
        width,
        height,
        buffers: [Some(b0), Some(b1)],
        front: 0,
        state: KmsLifecycle::Prepared,
        flip_pending: false,
        pending_front: 0,
        release_requested: false,
        crtc_restored: true,
        old_vt_mode,
        old_signal_mask,
        signals: Some(signals),
        owner_tid: current_tid(),
        _not_send_sync: PhantomData,
    })
}

fn capture_baseline(
    fd: c_int,
    connector: u32,
    crtc_id: u32,
    required_mode: ModeInfo,
) -> Result<(Crtc, [u32; MAX_OBJECTS], u32), KmsError> {
    let mut res = CardResources::default();
    call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count == 0
        || res.crtc_count as usize > MAX_OBJECTS
        || res.connector_count == 0
        || res.connector_count as usize > MAX_OBJECTS
        || res.encoder_count == 0
        || res.encoder_count as usize > MAX_OBJECTS
    {
        return Err(KmsError::UnsupportedTopology);
    }
    let mut crtc_ids = [0u32; MAX_OBJECTS];
    let mut conn_ids = [0u32; MAX_OBJECTS];
    let mut enc_ids = [0u32; MAX_OBJECTS];
    res.crtc_ptr = crtc_ids.as_mut_ptr() as u64;
    res.connector_ptr = conn_ids.as_mut_ptr() as u64;
    res.encoder_ptr = enc_ids.as_mut_ptr() as u64;
    call(fd, GETRESOURCES, &mut res)?;
    if res.crtc_count as usize > MAX_OBJECTS
        || res.connector_count as usize > MAX_OBJECTS
        || res.encoder_count as usize > MAX_OBJECTS
    {
        return Err(KmsError::UnsupportedTopology);
    }
    let crtc_index = crtc_ids
        .iter()
        .take(res.crtc_count as usize)
        .position(|id| *id == crtc_id)
        .ok_or(KmsError::UnsupportedTopology)?;

    let mut modes = [ModeInfo::default(); MAX_MODES];
    let mut encoder_ids = [0u32; MAX_OBJECTS];
    let mut selected = Connector {
        connector_id: connector,
        mode_count: MAX_MODES as u32,
        encoder_count: MAX_OBJECTS as u32,
        modes_ptr: modes.as_mut_ptr() as u64,
        encoders_ptr: encoder_ids.as_mut_ptr() as u64,
        ..Default::default()
    };
    call(fd, GETCONNECTOR, &mut selected)?;
    if selected.connection != CONNECTED
        || selected.mode_count as usize > MAX_MODES
        || selected.encoder_count as usize > MAX_OBJECTS
    {
        return Err(KmsError::NoConnectedMode);
    }
    if !modes
        .iter()
        .take(selected.mode_count as usize)
        .any(|mode| *mode == required_mode)
    {
        return Err(KmsError::NoConnectedMode);
    }
    let mut compatible = false;
    for &eid in encoder_ids.iter().take(selected.encoder_count as usize) {
        let mut encoder = Encoder {
            id: eid,
            ..Default::default()
        };
        call(fd, GETENCODER, &mut encoder)?;
        if encoder.possible_crtcs & (1u32 << crtc_index) != 0
            && (encoder.crtc_id == 0 || encoder.crtc_id == crtc_id)
        {
            compatible = true;
            break;
        }
    }
    if !compatible {
        return Err(KmsError::UnsupportedTopology);
    }

    let mut routed = [0u32; MAX_OBJECTS];
    let mut routed_count = 0u32;
    let mut one_mode = [ModeInfo::default(); 1];
    let mut route_encoders = [0u32; MAX_OBJECTS];
    for &cid in conn_ids.iter().take(res.connector_count as usize) {
        let mut c = Connector {
            connector_id: cid,
            mode_count: 1,
            encoder_count: MAX_OBJECTS as u32,
            modes_ptr: one_mode.as_mut_ptr() as u64,
            encoders_ptr: route_encoders.as_mut_ptr() as u64,
            ..Default::default()
        };
        call(fd, GETCONNECTOR, &mut c)?;
        if c.encoder_id != 0 {
            let mut encoder = Encoder {
                id: c.encoder_id,
                ..Default::default()
            };
            call(fd, GETENCODER, &mut encoder)?;
            if encoder.crtc_id == crtc_id {
                if cid != connector {
                    return Err(KmsError::UnsupportedTopology);
                }
                let slot = routed
                    .get_mut(routed_count as usize)
                    .ok_or(KmsError::UnsupportedTopology)?;
                *slot = cid;
                routed_count += 1;
            }
        }
    }
    let mut old_crtc = Crtc {
        crtc_id,
        ..Default::default()
    };
    call(fd, GETCRTC, &mut old_crtc)?;
    Ok((old_crtc, routed, routed_count))
}

fn query_active_vt(tty_fd: c_int) -> Result<u16, KmsError> {
    let mut state = VtState::default();
    call(tty_fd, VT_GETSTATE, &mut state)?;
    Ok(state.active)
}
fn current_tid() -> c_int {
    // SAFETY: gettid/getpid take no arguments and have no pointer invariants.
    unsafe { gettid() }
}
fn is_dedicated_owner_process() -> bool {
    // VT_PROCESS signals are process-directed. Requiring the main thread and one-task process
    // lets signalfd own SIGUSR1/SIGUSR2 without installing a global handler or sharing masks.
    // SAFETY: getpid takes no arguments and has no pointer invariants.
    if current_tid() != unsafe { getpid() } {
        return false;
    }
    // seal-allow: inspect the fixed process task directory to enforce single-threaded VT signal ownership
    std::fs::read_dir("/proc/self/task")
        .ok()
        .is_some_and(|tasks| tasks.count() == 1)
}
fn install_vt_process(tty_fd: c_int) -> Result<(File, SigSet), KmsError> {
    let mut mask = SigSet::default();
    // SAFETY: each pointer references writable SigSet storage matching libc's Linux sigset_t.
    let empty = unsafe { sigemptyset(&mut mask) };
    // SAFETY: `mask` is a live initialized sigset_t.
    let rel = unsafe { sigaddset(&mut mask, SIGUSR1) };
    // SAFETY: `mask` is a live initialized sigset_t.
    let acq = unsafe { sigaddset(&mut mask, SIGUSR2) };
    if empty != 0 || rel != 0 || acq != 0 {
        return Err(KmsError::SignalControl(-1));
    }
    let mut old_mask = SigSet::default();
    // SAFETY: pthread_sigmask reads `mask` and writes the previous mask to live storage.
    let mask_result = unsafe { pthread_sigmask(SIG_BLOCK, &mask, &mut old_mask) };
    if mask_result != 0 {
        return Err(KmsError::SignalControl(mask_result));
    }
    // SAFETY: signalfd copies the blocked-signal set; no pointers are retained.
    let signal_fd = unsafe { signalfd(-1, &mask, SFD_CLOEXEC | SFD_NONBLOCK) };
    if signal_fd < 0 {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        // SAFETY: restoring the exact mask captured above on this owner thread.
        let _ = unsafe { pthread_sigmask(SIG_SETMASK, &old_mask, core::ptr::null_mut()) };
        return Err(KmsError::SignalControl(error));
    }
    // SAFETY: signalfd returned a new owned fd; File assumes its sole close responsibility.
    let file = unsafe { File::from_raw_fd(signal_fd) };
    let mut pending_info = [0u8; SIGNAL_INFO_BYTES];
    // SAFETY: pending_info is a writable fixed-size signalfd_siginfo record buffer.
    let pending = unsafe {
        read(
            file.as_raw_fd(),
            pending_info.as_mut_ptr().cast(),
            pending_info.len(),
        )
    };
    if pending >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(11) {
        let error = if pending >= 0 {
            -1
        } else {
            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
        };
        drop(file);
        let _ = restore_signal_mask(&old_mask);
        return Err(KmsError::SignalControl(error));
    }
    let mut mode = VtMode {
        mode: VT_PROCESS,
        waitv: 0,
        relsig: SIGUSR1 as i16,
        acqsig: SIGUSR2 as i16,
        frsig: 0,
    };
    if let Err(e) = call(tty_fd, VT_SETMODE, &mut mode) {
        drop(file);
        // SAFETY: restoring the exact mask captured above on this owner thread.
        let _ = unsafe { pthread_sigmask(SIG_SETMASK, &old_mask, core::ptr::null_mut()) };
        return Err(e);
    }
    Ok((file, old_mask))
}
fn drain_signals(fd: c_int) -> Result<(), KmsError> {
    let mut info = [0u8; SIGNAL_INFO_BYTES];
    for _ in 0..MAX_INTERRUPTS {
        // SAFETY: info is writable signalfd_siginfo-sized storage; each read is bounded.
        let n = unsafe { read(fd, info.as_mut_ptr().cast(), info.len()) };
        if n == SIGNAL_INFO_BYTES as isize {
            continue;
        }
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(11) {
            return Ok(());
        }
        if n < 0 {
            return Err(KmsError::SignalControl(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
            ));
        }
        return Err(KmsError::SignalControl(-1));
    }
    Err(KmsError::SignalControl(-1))
}
fn restore_signal_mask(mask: &SigSet) -> Result<(), KmsError> {
    // SAFETY: restores the exact valid mask saved by pthread_sigmask during signal setup.
    let rc = unsafe { pthread_sigmask(SIG_SETMASK, mask, core::ptr::null_mut()) };
    if rc != 0 {
        Err(KmsError::SignalControl(rc))
    } else {
        Ok(())
    }
}
fn control_signals_blocked() -> bool {
    let mut current = SigSet::default();
    // SAFETY: a null set asks pthread_sigmask only to copy this thread's current mask out.
    let rc = unsafe { pthread_sigmask(SIG_SETMASK, core::ptr::null(), &mut current) };
    if rc != 0 {
        return false;
    }
    // SAFETY: `current` is the initialized mask returned by pthread_sigmask.
    let release = unsafe { sigismember(&current, SIGUSR1) };
    // SAFETY: `current` is the initialized mask returned by pthread_sigmask.
    let acquire = unsafe { sigismember(&current, SIGUSR2) };
    release == 1 && acquire == 1
}
fn ioctl_value(fd: c_int, request: c_ulong, value: c_ulong) -> Result<(), KmsError> {
    // SAFETY: VT_RELDISP uses a small argument-by-value command (release-ok or ack-acquire).
    let rc = unsafe { ioctl(fd, request, value) };
    if rc < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

impl KmsDisplay {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn lifecycle(&self) -> KmsLifecycle {
        self.state
    }
    pub fn must_terminate_owner(&self) -> bool {
        matches!(
            self.state,
            KmsLifecycle::Faulted | KmsLifecycle::FaultedReleaseAckPending
        )
    }
    /// Enter graphics mode and set the selected CRTC to the zeroed front buffer.
    pub fn start(&mut self) -> Result<(), KmsError> {
        self.ensure_owner()?;
        self.service_vt(0)?;
        if self.state != KmsLifecycle::Prepared {
            return Err(KmsError::InvalidLifecycle);
        }
        self.ensure_active()?;
        if let Err(e) = self.set_kd_mode(KD_GRAPHICS) {
            if let Err(rollback) = self.set_kd_mode(self.old_kd_mode) {
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::CleanupFailed {
                    operation: system_code(e),
                    rollback: system_code(rollback),
                });
            }
            return Err(e);
        }
        if let Err(e) = self.service_vt(0) {
            if let Err(rollback) = self.set_kd_mode(self.old_kd_mode) {
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::CleanupFailed {
                    operation: system_code(e),
                    rollback: system_code(rollback),
                });
            }
            return Err(e);
        }
        if self.state != KmsLifecycle::Prepared {
            return Err(KmsError::InactiveVt);
        }
        let front = match self.buffers.first().and_then(Option::as_ref) {
            Some(front) => front,
            None => {
                if self.set_kd_mode(self.old_kd_mode).is_err() {
                    self.state = KmsLifecycle::Faulted;
                }
                return Err(KmsError::UnsupportedTopology);
            }
        };
        let mut crtc = Crtc {
            connectors_ptr: (&self.connector as *const u32) as u64,
            connector_count: 1,
            crtc_id: self.crtc_id,
            fb_id: front.fb,
            mode_valid: 1,
            mode: self.mode,
            ..Default::default()
        };
        if let Err(e) = call(self.drm.as_raw_fd(), SETCRTC, &mut crtc) {
            let mut old = self.old_crtc;
            old.connectors_ptr = self.old_connectors.as_ptr() as u64;
            old.connector_count = self.old_connector_count;
            let crtc_restore = call(self.drm.as_raw_fd(), SETCRTC, &mut old);
            let tty_restore = self.set_kd_mode(self.old_kd_mode);
            self.crtc_restored = crtc_restore.is_ok();
            if crtc_restore.is_err() || tty_restore.is_err() {
                self.state = KmsLifecycle::Faulted;
                let rollback = crtc_restore
                    .err()
                    .or_else(|| tty_restore.err())
                    .unwrap_or(KmsError::RestoreFailed);
                return Err(KmsError::CleanupFailed {
                    operation: system_code(e),
                    rollback: system_code(rollback),
                });
            }
            return Err(e);
        }
        self.crtc_restored = false;
        self.state = kms_transition(self.state, KmsTransition::Start)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        self.front = 0;
        Ok(())
    }
    /// Copy the whole validated frame into the back buffer and flip at vblank; returns only
    /// after the kernel's DRM_EVENT_FLIP_COMPLETE event has been read.
    pub fn present(&mut self, frame: PixelFrame<'_>) -> Result<(), KmsError> {
        self.ensure_owner()?;
        self.service_vt(0)?;
        if self.state != KmsLifecycle::Active {
            return Err(KmsError::InvalidLifecycle);
        }
        self.ensure_active()?;
        let back = 1 - self.front;
        let buffer = self
            .buffers
            .get_mut(back)
            .and_then(Option::as_mut)
            .ok_or(KmsError::UnsupportedTopology)?;
        buffer.copy_frame(frame, self.height, self.width)?;
        let fb_id = buffer.fb;
        self.service_vt(0)?;
        if self.state != KmsLifecycle::Active {
            return Err(KmsError::InactiveVt);
        }
        let mut flip = PageFlip {
            crtc_id: self.crtc_id,
            fb_id,
            flags: FLIP_EVENT,
            ..Default::default()
        };
        call(self.drm.as_raw_fd(), PAGE_FLIP, &mut flip)?;
        self.state = kms_transition(self.state, KmsTransition::FlipSubmitted)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        self.flip_pending = true;
        self.pending_front = back;
        self.wait_pending_flip(WAIT_MS as u32)?;
        if self.state == KmsLifecycle::Active {
            Ok(())
        } else {
            Err(KmsError::InactiveVt)
        }
    }
    /// Service VT release/acquire signals. Every wait is capped at one second; a timed out
    /// pending flip remains owned and blocks future writes until completed or fault-released.
    pub fn service_vt(&mut self, timeout_ms: u32) -> Result<KmsLifecycle, KmsError> {
        self.ensure_owner()?;
        if timeout_ms > MAX_INPUT_WAIT_MS {
            return Err(KmsError::InvalidLifecycle);
        }
        if self.must_terminate_owner() {
            return Err(KmsError::Faulted);
        }
        if self.flip_pending {
            self.wait_pending_flip(timeout_ms)?;
            return Ok(self.state);
        }
        match self.read_signal(Some(timeout_ms))? {
            Some(SIGUSR1) => {
                self.begin_release()?;
            }
            Some(SIGUSR2) => {
                self.begin_acquire()?;
            }
            Some(_) => return Err(KmsError::SignalControl(-1)),
            None => {}
        }
        Ok(self.state)
    }
    fn begin_release(&mut self) -> Result<(), KmsError> {
        self.state = kms_transition(self.state, KmsTransition::ReleaseSignal)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        self.release_requested = true;
        if self.flip_pending {
            match self.wait_pending_flip(WAIT_MS as u32) {
                Ok(()) => {}
                Err(KmsError::Timeout) => return self.fault_release_pending_flip(),
                Err(e) => return Err(e),
            }
        }
        if self.state == KmsLifecycle::ReleasePending {
            self.complete_release()
        } else {
            Ok(())
        }
    }
    fn wait_pending_flip(&mut self, timeout_ms: u32) -> Result<(), KmsError> {
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let mut interruptions = 0u8;
        let mut zero_timeout_probed = false;
        while self.flip_pending {
            let now = Instant::now();
            let expired = now >= deadline;
            if expired && (timeout_ms != 0 || zero_timeout_probed) {
                if self.release_requested {
                    return self.fault_release_pending_flip();
                }
                return Err(KmsError::Timeout);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let millis = if expired {
                0
            } else {
                remaining.as_millis().min(i32::MAX as u128) as c_int
            };
            let signal_fd = self
                .signals
                .as_ref()
                .ok_or(KmsError::SignalControl(-1))?
                .as_raw_fd();
            let mut fds = [
                PollFd {
                    fd: self.drm.as_raw_fd(),
                    events: POLLIN,
                    revents: 0,
                },
                PollFd {
                    fd: signal_fd,
                    events: POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: fds is a two-element initialized poll array valid for the duration of poll.
            let count = unsafe { poll(fds.as_mut_ptr(), fds.len(), millis) };
            if timeout_ms == 0 {
                zero_timeout_probed = true;
            }
            if count == 0 {
                if self.release_requested {
                    return self.fault_release_pending_flip();
                }
                return Err(KmsError::Timeout);
            }
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    interruptions = interruptions.saturating_add(1);
                    if interruptions <= MAX_INTERRUPTS {
                        continue;
                    }
                }
                if self.release_requested {
                    return self.fault_release_pending_flip();
                }
                self.state = KmsLifecycle::Faulted;
                return Err(sys_error(error));
            }
            if fds.get(1).is_some_and(|fd| fd.revents & POLLIN != 0) {
                match self.read_signal(None)? {
                    Some(SIGUSR1) if !self.release_requested => {
                        self.state = kms_transition(self.state, KmsTransition::ReleaseSignal)
                            .map_err(|_| KmsError::InvalidLifecycle)?;
                        self.release_requested = true;
                    }
                    Some(_) => {
                        self.state = KmsLifecycle::Faulted;
                        return Err(KmsError::SignalControl(-1));
                    }
                    None => {}
                }
            }
            if fds.first().is_some_and(|fd| fd.revents & POLLIN != 0) {
                match read_flip_event(self.drm.as_raw_fd()) {
                    Err(KmsError::System(11)) => continue,
                    Err(_) if self.release_requested => return self.fault_release_pending_flip(),
                    Err(e) => {
                        self.state = KmsLifecycle::Faulted;
                        return Err(e);
                    }
                    Ok(FlipRead::Other) => {}
                    Ok(FlipRead::Complete) => {
                        self.flip_pending = false;
                        self.front = self.pending_front;
                        self.state = kms_transition(self.state, KmsTransition::FlipCompleted)
                            .map_err(|_| KmsError::InvalidLifecycle)?;
                    }
                }
            } else if fds
                .first()
                .is_some_and(|fd| fd.revents & (POLLERR | POLLHUP | POLLNVAL) != 0)
            {
                if self.release_requested {
                    return self.fault_release_pending_flip();
                }
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::MalformedEvent);
            }
            if fds
                .get(1)
                .is_some_and(|fd| fd.revents & (POLLERR | POLLHUP | POLLNVAL) != 0)
            {
                if self.release_requested {
                    return self.fault_release_pending_flip();
                }
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::SignalControl(-1));
            }
            if !self.flip_pending && self.release_requested {
                return self.complete_release();
            }
        }
        Ok(())
    }
    fn complete_release(&mut self) -> Result<(), KmsError> {
        let restore = self.restore_crtc();
        let text = self.set_kd_mode(KD_TEXT);
        if restore.is_err() || text.is_err() {
            let e = restore
                .err()
                .or_else(|| text.err())
                .unwrap_or(KmsError::RestoreFailed);
            self.state = kms_transition(self.state, KmsTransition::RestoreFailed)
                .map_err(|_| KmsError::InvalidLifecycle)?;
            let drop_master = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
            let release = ioctl_value(self.tty.as_raw_fd(), VT_RELDISP, VT_RELEASE_OK);
            if release.is_ok() {
                self.state = kms_transition(self.state, KmsTransition::ReleaseAcknowledged)
                    .map_err(|_| KmsError::InvalidLifecycle)?;
            }
            drop_master?;
            if let Err(r) = release {
                return Err(KmsError::ReleaseFailed(system_code(r)));
            }
            return Err(e);
        }
        self.state = kms_transition(self.state, KmsTransition::RestoreSucceeded)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        let drop_result = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
        if let Err(e) = drop_result {
            self.state = kms_transition(self.state, KmsTransition::DropMasterFailed)
                .map_err(|_| KmsError::InvalidLifecycle)?;
            let release = ioctl_value(self.tty.as_raw_fd(), VT_RELDISP, VT_RELEASE_OK);
            if release.is_ok() {
                self.state = kms_transition(self.state, KmsTransition::ReleaseAcknowledged)
                    .map_err(|_| KmsError::InvalidLifecycle)?;
            }
            return Err(e);
        }
        if let Err(e) = ioctl_value(self.tty.as_raw_fd(), VT_RELDISP, VT_RELEASE_OK) {
            self.state = kms_transition(self.state, KmsTransition::ReleaseAckFailed)
                .map_err(|_| KmsError::InvalidLifecycle)?;
            return Err(KmsError::ReleaseFailed(system_code(e)));
        }
        self.state = kms_transition(self.state, KmsTransition::ReleaseAcknowledged)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        self.release_requested = false;
        Ok(())
    }
    fn fault_release_pending_flip(&mut self) -> Result<(), KmsError> {
        self.state = kms_transition(self.state, KmsTransition::ReleaseDeadlineExpired)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        let drop_master = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
        let release = ioctl_value(self.tty.as_raw_fd(), VT_RELDISP, VT_RELEASE_OK);
        if release.is_ok() {
            self.state = kms_transition(self.state, KmsTransition::ReleaseAcknowledged)
                .map_err(|_| KmsError::InvalidLifecycle)?;
        }
        drop_master?;
        if let Err(e) = release {
            return Err(KmsError::ReleaseFailed(system_code(e)));
        }
        Err(KmsError::Faulted)
    }
    fn begin_acquire(&mut self) -> Result<(), KmsError> {
        self.state = kms_transition(self.state, KmsTransition::AcquireSignal)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        if query_active_vt(self.tty.as_raw_fd()) != Ok(self.active_vt) {
            self.state = KmsLifecycle::Faulted;
            return Err(KmsError::InactiveVt);
        }
        // The kernel delivers SIGUSR2 only after this VT is active; VT_RELDISP(2) is
        // meaningful only for acknowledging a still-pending acquire notification. This
        // process is already active, so avoid a redundant ACK that can race another release.
        if let Err(e) = ioctl_call_unit(self.drm.as_raw_fd(), SET_MASTER) {
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        let baseline = capture_baseline(
            self.drm.as_raw_fd(),
            self.connector,
            self.crtc_id,
            self.mode,
        );
        let (old_crtc, old_connectors, old_connector_count) = match baseline {
            Ok(v) => v,
            Err(e) => {
                let _ = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
                self.state = KmsLifecycle::Faulted;
                return Err(e);
            }
        };
        self.old_crtc = old_crtc;
        self.old_connectors = old_connectors;
        self.old_connector_count = old_connector_count;
        self.crtc_restored = true;
        if let Err(e) = self.set_kd_mode(KD_GRAPHICS) {
            let _ = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        let front = match self.buffers.get(self.front).and_then(Option::as_ref) {
            Some(front) => front,
            None => {
                let _ = self.set_kd_mode(KD_TEXT);
                let _ = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::UnsupportedTopology);
            }
        };
        let mut crtc = Crtc {
            connectors_ptr: (&self.connector as *const u32) as u64,
            connector_count: 1,
            crtc_id: self.crtc_id,
            fb_id: front.fb,
            mode_valid: 1,
            mode: self.mode,
            ..Default::default()
        };
        if let Err(e) = call(self.drm.as_raw_fd(), SETCRTC, &mut crtc) {
            let mut old = self.old_crtc;
            old.connectors_ptr = self.old_connectors.as_ptr() as u64;
            old.connector_count = self.old_connector_count;
            self.crtc_restored = call(self.drm.as_raw_fd(), SETCRTC, &mut old).is_ok();
            let _ = self.set_kd_mode(KD_TEXT);
            let _ = ioctl_call_unit(self.drm.as_raw_fd(), DROP_MASTER);
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        self.crtc_restored = false;
        self.state = kms_transition(self.state, KmsTransition::AcquireSucceeded)
            .map_err(|_| KmsError::InvalidLifecycle)?;
        Ok(())
    }
    fn ensure_active(&self) -> Result<(), KmsError> {
        if query_active_vt(self.tty.as_raw_fd())? != self.active_vt {
            return Err(KmsError::InactiveVt);
        }
        Ok(())
    }
    fn ensure_owner(&self) -> Result<(), KmsError> {
        if current_tid() != self.owner_tid
            || !is_dedicated_owner_process()
            || !control_signals_blocked()
        {
            Err(KmsError::DedicatedOwnerProcessRequired)
        } else {
            Ok(())
        }
    }
    fn read_signal(&mut self, timeout: Option<u32>) -> Result<Option<c_int>, KmsError> {
        let fd = self
            .signals
            .as_ref()
            .ok_or(KmsError::SignalControl(-1))?
            .as_raw_fd();
        if let Some(ms) = timeout {
            let mut pfd = PollFd {
                fd,
                events: POLLIN,
                revents: 0,
            };
            // SAFETY: pfd points to one initialized pollfd for the duration of poll.
            let ready = unsafe { poll(&mut pfd, 1, ms as c_int) };
            if ready == 0 {
                return Ok(None);
            }
            if ready < 0 {
                return Err(last_error());
            }
            if pfd.revents & POLLIN == 0 {
                return Err(KmsError::SignalControl(-1));
            }
        }
        let mut info = [0u8; SIGNAL_INFO_BYTES];
        // SAFETY: info is a writable fixed-size signalfd_siginfo record buffer.
        let n = unsafe { read(fd, info.as_mut_ptr().cast(), info.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(11) {
                return Ok(None);
            }
            return Err(KmsError::SignalControl(e.raw_os_error().unwrap_or(-1)));
        }
        if n as usize != SIGNAL_INFO_BYTES {
            return Err(KmsError::SignalControl(-1));
        }
        decode_vt_signal_info(&info).map(Some)
    }
    fn set_kd_mode(&self, mode: c_int) -> Result<(), KmsError> {
        // SAFETY: KDSETMODE is an argument-by-value ioctl (`KD_TEXT` or `KD_GRAPHICS`).
        let rc = unsafe { ioctl(self.tty.as_raw_fd(), KDSETMODE, mode as c_ulong) };
        if rc < 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    /// Restore the saved CRTC, VT mode, signal mask, buffers, then relinquish ownership.
    pub fn close(mut self) -> Result<(), KmsError> {
        self.cleanup()
    }
    fn cleanup(&mut self) -> Result<(), KmsError> {
        if self.state == KmsLifecycle::Closed {
            return Ok(());
        }
        self.ensure_owner()?;
        if self.must_terminate_owner() {
            return Err(KmsError::Faulted);
        }
        if self.flip_pending {
            self.state = KmsLifecycle::Faulted;
            return Err(KmsError::Timeout);
        }
        let previous = self.state;
        if !kms_cleanup_may_destroy(previous, self.flip_pending, self.crtc_restored)
            && previous != KmsLifecycle::Active
        {
            self.state = KmsLifecycle::Faulted;
            return Err(KmsError::InvalidLifecycle);
        }
        let has_master = previous != KmsLifecycle::Suspended;
        let final_state = if previous == KmsLifecycle::Active {
            kms_transition(previous, KmsTransition::CloseRestored)
        } else {
            kms_transition(previous, KmsTransition::Close)
        }
        .map_err(|_| KmsError::InvalidLifecycle)?;
        if previous == KmsLifecycle::Active {
            if self.ensure_active().is_err() {
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::InactiveVt);
            }
            if let Err(e) = self.restore_crtc() {
                self.state = KmsLifecycle::Faulted;
                return Err(e);
            }
            if let Err(e) = self.set_kd_mode(self.old_kd_mode) {
                self.state = KmsLifecycle::Faulted;
                return Err(e);
            }
            if !kms_cleanup_may_destroy(previous, self.flip_pending, self.crtc_restored) {
                self.state = KmsLifecycle::Faulted;
                return Err(KmsError::RestoreFailed);
            }
        } else if previous != KmsLifecycle::Prepared && previous != KmsLifecycle::Suspended {
            self.state = KmsLifecycle::Faulted;
            return Err(KmsError::InvalidLifecycle);
        }
        let fd = self.drm.as_raw_fd();
        for slot in &mut self.buffers {
            if let Some(b) = slot.take() {
                let result = if previous == KmsLifecycle::Suspended {
                    unmap_buffer(&b)
                } else {
                    destroy_buffer(fd, b)
                };
                if let Err(e) = result {
                    self.state = KmsLifecycle::Faulted;
                    return Err(e);
                }
            }
        }
        if has_master {
            if let Err(e) = ioctl_call_unit(fd, DROP_MASTER) {
                self.state = KmsLifecycle::Faulted;
                return Err(e);
            }
        }
        let mut old_mode = self.old_vt_mode;
        if let Err(e) = call(self.tty.as_raw_fd(), VT_SETMODE, &mut old_mode) {
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        if let Err(e) = drain_signals(
            self.signals
                .as_ref()
                .ok_or(KmsError::SignalControl(-1))?
                .as_raw_fd(),
        ) {
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        if let Some(signals) = self.signals.take() {
            drop(signals);
        }
        if let Err(e) = restore_signal_mask(&self.old_signal_mask) {
            self.state = KmsLifecycle::Faulted;
            return Err(e);
        }
        self.state = final_state;
        Ok(())
    }
}
fn decode_vt_signal_info(info: &[u8]) -> Result<c_int, KmsError> {
    let signo = u32::from_ne_bytes(
        info.get(0..4)
            .ok_or(KmsError::SignalControl(-1))?
            .try_into()
            .map_err(|_| KmsError::SignalControl(-1))?,
    );
    let code = i32::from_ne_bytes(
        info.get(8..12)
            .ok_or(KmsError::SignalControl(-1))?
            .try_into()
            .map_err(|_| KmsError::SignalControl(-1))?,
    );
    match i32::try_from(signo).ok() {
        Some(SIGUSR1 | SIGUSR2) if code == SI_KERNEL => {
            i32::try_from(signo).map_err(|_| KmsError::UnexpectedSignal(-1))
        }
        Some(other) => Err(KmsError::UnexpectedSignal(other)),
        None => Err(KmsError::UnexpectedSignal(-1)),
    }
}
impl KmsDisplay {
    fn restore_crtc(&mut self) -> Result<(), KmsError> {
        let mut old = self.old_crtc;
        old.connectors_ptr = self.old_connectors.as_ptr() as u64;
        old.connector_count = self.old_connector_count;
        call(self.drm.as_raw_fd(), SETCRTC, &mut old)?;
        self.crtc_restored = true;
        Ok(())
    }
}
impl Drop for KmsDisplay {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn destroy_buffer(fd: c_int, b: Buffer) -> Result<(), KmsError> {
    let mut failure = unmap_buffer(&b).err();
    let mut fb = b.fb;
    if let Err(e) = ioctl_arg(fd, RMFB, &mut fb) {
        if failure.is_none() {
            failure = Some(e);
        }
    }
    let mut d = DestroyDumb { handle: b.handle };
    if let Err(e) = call(fd, DESTROY_DUMB, &mut d) {
        if failure.is_none() {
            failure = Some(e);
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
fn unmap_buffer(b: &Buffer) -> Result<(), KmsError> {
    // SAFETY: mapping pointer/length are the successful mmap region owned by this Buffer.
    let rc = unsafe { munmap(b.map.as_ptr().cast(), b.size) };
    if rc < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}
enum FlipRead {
    Complete,
    Other,
}
fn read_flip_event(fd: c_int) -> Result<FlipRead, KmsError> {
    let mut events = [0u8; 256];
    // SAFETY: events is writable fixed-size storage and read is bounded to that length.
    let got = unsafe { read(fd, events.as_mut_ptr().cast(), events.len()) };
    if got < 0 {
        return Err(last_error());
    }
    if got == 0 {
        return Err(KmsError::MalformedEvent);
    }
    let mut offset = 0usize;
    let mut complete = false;
    while offset < got as usize {
        let header = events
            .get(offset..offset.checked_add(8).ok_or(KmsError::MalformedEvent)?)
            .ok_or(KmsError::MalformedEvent)?;
        let kind = u32::from_ne_bytes(
            header
                .get(0..4)
                .ok_or(KmsError::MalformedEvent)?
                .try_into()
                .map_err(|_| KmsError::MalformedEvent)?,
        );
        let len = usize::try_from(u32::from_ne_bytes(
            header
                .get(4..8)
                .ok_or(KmsError::MalformedEvent)?
                .try_into()
                .map_err(|_| KmsError::MalformedEvent)?,
        ))
        .map_err(|_| KmsError::MalformedEvent)?;
        if len < 8
            || offset
                .checked_add(len)
                .filter(|end| *end <= got as usize)
                .is_none()
        {
            return Err(KmsError::MalformedEvent);
        }
        if kind == EVENT_FLIP_COMPLETE {
            if len < FLIP_EVENT_MIN_BYTES {
                return Err(KmsError::MalformedEvent);
            }
            complete = true;
        }
        offset += len;
    }
    if complete {
        Ok(FlipRead::Complete)
    } else {
        Ok(FlipRead::Other)
    }
}

// Safe wrapper: each call's pointer is to a live repr(C) UAPI structure of the request's exact size.
fn call<T>(fd: c_int, request: c_ulong, value: &mut T) -> Result<(), KmsError> {
    // SAFETY: callers pass the matching repr(C) UAPI record, initialized and alive through ioctl.
    let rc = unsafe { ioctl(fd, request, value as *mut T) };
    if rc < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}
fn call_unit(fd: c_int, request: c_ulong) -> Result<(), KmsError> {
    ioctl_call_unit(fd, request)
}
fn ioctl_call_unit(fd: c_int, request: c_ulong) -> Result<(), KmsError> {
    // SAFETY: request is DRM_IOCTL_SET_MASTER or DROP_MASTER, both argument-free UAPI ioctls.
    let rc = unsafe { ioctl(fd, request) };
    if rc < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}
fn ioctl_arg(fd: c_int, request: c_ulong, value: &mut u32) -> Result<(), KmsError> {
    // SAFETY: RMFB takes a pointer to one unsigned framebuffer ID.
    let rc = unsafe { ioctl(fd, request, value as *mut u32) };
    if rc < 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}
fn last_error() -> KmsError {
    KmsError::System(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))
}
fn system_code(error: KmsError) -> i32 {
    match error {
        KmsError::System(code) => code,
        _ => -1,
    }
}
fn sys_error(e: std::io::Error) -> KmsError {
    KmsError::System(e.raw_os_error().unwrap_or(-1))
}
fn valid_card_path(s: &str) -> bool {
    s.strip_prefix("/dev/dri/card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
fn valid_tty_path(s: &str) -> bool {
    tty_number(s).is_some()
}
fn tty_number(s: &str) -> Option<u16> {
    let n = s.strip_prefix("/dev/tty")?;
    let x = n.parse::<u16>().ok()?;
    (x > 0 && x < 16).then_some(x)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    #[test]
    fn uapi_layouts_match_linux_64_bit_abi() {
        assert_eq!(size_of::<ModeInfo>(), 68);
        assert_eq!(size_of::<CardResources>(), 64);
        assert_eq!(size_of::<Crtc>(), 104);
        assert_eq!(size_of::<Connector>(), 80);
        assert_eq!(size_of::<CreateDumb>(), 32);
        assert_eq!(size_of::<PageFlip>(), 24);
        assert_eq!(GETRESOURCES, 0xC04064A0);
        assert_eq!(GETCONNECTOR, 0xC05064A7);
        assert_eq!(EVIOCGVERSION, 0x80044501);
    }
    #[test]
    fn explicit_device_paths_only() {
        assert!(valid_card_path("/dev/dri/card0"));
        assert!(!valid_card_path("/dev/dri/renderD128"));
        assert!(valid_tty_path("/dev/tty7"));
        assert!(!valid_tty_path("/dev/tty17"));
        assert!(!valid_tty_path("/dev/ttyS0"));
    }
    #[test]
    fn only_kernel_generated_vt_signals_drive_lifecycle() {
        let mut info = [0u8; SIGNAL_INFO_BYTES];
        info[0..4].copy_from_slice(&(SIGUSR1 as u32).to_ne_bytes());
        info[8..12].copy_from_slice(&SI_KERNEL.to_ne_bytes());
        assert_eq!(decode_vt_signal_info(&info), Ok(SIGUSR1));
        info[8..12].copy_from_slice(&SI_USER.to_ne_bytes());
        assert_eq!(
            decode_vt_signal_info(&info),
            Err(KmsError::UnexpectedSignal(SIGUSR1))
        );
    }
    #[test]
    fn plain_input_records_have_explicit_zero_padding() {
        let record = input_record(
            InputRecordKind::SyncReport,
            InputEvent::default(),
            crate::input_snapshot_new(),
        );
        assert_eq!(record.reserved, [0; 7]);
        assert_eq!(record.snapshot.reserved, [0; 3]);
    }
}

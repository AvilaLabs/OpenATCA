# oa-display-present

Linux-only direct display/input shell in the product workspace. This crate has no dependencies.
Its pure surface validates bounded XRGB8888 frame extents and
decodes Linux 64-bit evdev records into fixed-capacity plain input records.

`open_kms("/dev/dri/cardN", "/dev/ttyN", connector_id)` verifies the selected VT is active,
in text mode, and in automatic VT-switch mode before opening DRM. It discovers read-only
topology, acquires DRM master, then recaptures the CRTC and connector routing under master.
After preparation it changes the VT to `VT_PROCESS` with reserved SIGUSR1/SIGUSR2 signals,
blocked and consumed through signalfd; it installs no global signal handler. Only kernel-origin
VT signals (`si_code == SI_KERNEL`) are accepted as handshakes; externally-sent SIGUSR1/SIGUSR2
returns `UnexpectedSignal` without changing display state. The kernel sends VT process-control
signals through privileged `kill_pid(..., 1)`, which yields `SI_KERNEL`. The API requires a
dedicated single-threaded owner process, and checks that contract at open and on each operation.
`start()` is the explicit modeset point. It allocates two bounded, linear dumb XRGB8888 buffers,
modesets the blank front buffer, then `present()` validates and copies every visible source row
to the back buffer, submits a legacy vblank page flip, and waits up to one second for
`DRM_EVENT_FLIP_COMPLETE`. `service_vt(timeout_ms)` handles VT release/acquire handshakes; release
restores the baseline before `VT_RELDISP(1)`, while acquire acknowledges, reacquires DRM master,
revalidates topology, recaptures the baseline, then re-enters graphics mode.

If a pending flip does not complete by the release deadline, the adapter stops writes, drops DRM
master, acknowledges VT release, and enters `Faulted` without claiming CRTC restoration or
destroying buffers. The dedicated owner process must exit on `must_terminate_owner()` so the
kernel closes the DRM file and its resources. A failed restore follows the same fault path. The
supervisor must detect the failed presenter and start its fallback. `close()` restores the CRTC,
VT mode and signal mask before buffer cleanup when the lifecycle is healthy. Multiple routed
connectors on the selected CRTC, unsupported dimensions, inactive VT, and unsupported topology
fail closed. No desktop, window system, or toolkit backend is used.
If setup cannot restore `VT_AUTO` after installing `VT_PROCESS`, `open_kms` returns
`OwnerMustTerminate` while retaining blocked signal ownership; the owner process must exit before
the supervisor retries.

This is an unqualified implementation: no DRM/VT ioctl or device was exercised here. Kernel
rejection of a restore or page-flip completion is reported as loss of display availability; the
faulted owner process must exit for kernel file cleanup. Do not claim hardware qualification
until tested on each target GPU/driver and console setup.

`validate_frame` accepts at most 4096 x 2160, 64 MiB, four-byte aligned rows, and checks the
whole `stride * height` extent before use. Pixel order is XRGB8888 bytes in memory (B,G,R,X on
little-endian Linux). `decode_evdev_event` expects exactly the 64-bit Linux UAPI `input_event`
layout (native-endian seconds/useconds, type, code, value). `open_evdev("/dev/input/eventN")`
reads one record at a time with a caller-supplied timeout capped at one second. `SYN_DROPPED`
emits `SyncLost`; subsequent records are marked `Discarded` until `SYN_REPORT`, where EVIOCGKEY
and EVIOCGABS return fixed key and absolute-axis state in `Resynchronized`. The snapshot marks
relative motion as lost because relative position cannot be queried. The consumer must invalidate
action state on `SyncLost` and allow actions only after `Resynchronized`, with pointer actions held
until a fresh position is established. `input_event_is_action_press` accepts EV_KEY value 1 and
excludes repeat value 2, so repeated keys cannot create another command edge.
Disconnects, partial records, and malformed timestamps are explicit errors. `InputBatch` has
128 struct-of-arrays records and counts refused pushes. Public fixed records include explicit
zero-valued reserved bytes so their C layouts have no implicit padding.

## ABI provenance

Layout and constant planning used the locally installed Linux UAPI headers
`/usr/include/linux/input.h`, `/usr/include/linux/vt.h`, `/usr/include/linux/kd.h`,
`/usr/include/drm/drm.h`, `/usr/include/drm/drm_mode.h`, and
`/usr/include/drm/drm_fourcc.h` (kernel DRM UAPI). Linux UAPI ioctl layouts and constants follow
these local headers. VT process-control values follow `/usr/include/linux/vt.h`; evdev key-state
query follows `EVIOCGKEY` in `/usr/include/linux/input.h`. The evdev decoder follows the 64-bit
`struct input_event`; it is not portable to 32-bit ABI. No DRM ioctl has been exercised here.
The KMS code uses legacy GETRESOURCES/GETCONNECTOR/GETENCODER/GETCRTC, CREATE_DUMB,
MAP_DUMB, ADDFB, SETCRTC, PAGE_FLIP, RMFB, and DESTROY_DUMB UAPI calls. Dumb-buffer layout is
linear and implicit; modifiers and atomic KMS are refused by omission. The upstream source is
the Linux kernel UAPI under `include/uapi/linux/`; hardware behavior must be qualified on the
target GPU/driver and console.
The connected-status value (`1`) follows the kernel's `connector_status_connected` enum in
[`include/drm/drm_connector.h`](https://github.com/torvalds/linux/blob/master/include/drm/drm_connector.h).
The kernel's [DRM KMS documentation](https://docs.kernel.org/gpu/drm-kms.html) describes dumb
buffers, legacy modesetting and page-flip synchronization; [DRM UAPI documentation](https://docs.kernel.org/gpu/drm-uapi.html)
describes event reads. The VT modes and ioctl values follow the upstream
[`include/uapi/linux/vt.h`](https://github.com/torvalds/linux/blob/master/include/uapi/linux/vt.h);
the release/acknowledge handling is implemented in
[`drivers/tty/vt/vt_ioctl.c`](https://github.com/torvalds/linux/blob/master/drivers/tty/vt/vt_ioctl.c).
That source passes `1` to `kill_pid` for release/acquire notifications; the kernel's
[`include/linux/sched/signal.h`](https://github.com/torvalds/linux/blob/master/include/linux/sched/signal.h)
defines `SEND_SIG_PRIV`, and the
[`include/uapi/asm-generic/siginfo.h`](https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/siginfo.h)
defines `SI_KERNEL` for privileged kernel-generated signals.
The DRM ioctl structures and commands follow upstream
[`include/uapi/drm/drm_mode.h`](https://github.com/torvalds/linux/blob/master/include/uapi/drm/drm_mode.h)
and the installed headers listed above.

## Validation

The source includes unit tests for frame-bound checks, event decoding, ioctl numbers/layouts, and
explicit device path validation. They were not run in this delegated change; the coordinating
agent owns builds, tests, formatting, and workspace integration. This crate has not been tested
on hardware.

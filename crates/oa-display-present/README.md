# oa-display-present

Linux-only direct display/input foundation. This crate has no dependencies and is deliberately
not yet in the product workspace. Its pure surface validates bounded XRGB8888 frame extents and
decodes Linux 64-bit evdev records into fixed-capacity plain input records.

`open_kms("/dev/dri/cardN", "/dev/ttyN")` verifies that the selected VT is active, in text
mode, and in automatic VT-switch mode before opening the DRM node and acquiring DRM master.
`start()` is the explicit modeset point. The adapter selects a connected connector, preferred
mode (or first advertised mode), and compatible encoder/CRTC under fixed resource limits; it
saves the CRTC and currently routed connectors first. It allocates two bounded, linear dumb
XRGB8888 buffers, modesets the blank front buffer, then `present()` validates and copies every
visible source row to the back buffer, submits a legacy vblank page flip, and waits up to one
second for `DRM_EVENT_FLIP_COMPLETE`. `close()` attempts CRTC/VT restoration before framebuffer
and dumb-buffer cleanup. `Drop` makes the same best effort. Multiple already-routed connectors
on the selected CRTC, unexpected active routing, unsupported dimensions, inactive VT, and
unsupported topology fail closed. There is no desktop, window system, or toolkit backend.

This is an unqualified implementation: no DRM/VT ioctl or device was exercised here, and the
restore path cannot guarantee recovery if the kernel/driver refuses restoration or the VT is
switched away while the adapter is active. The caller must keep the selected VT foreground and
must treat any returned error as a loss of display availability. Do not claim hardware
qualification until tested on each target GPU/driver and console setup.

`validate_frame` accepts at most 4096 x 2160, 64 MiB, four-byte aligned rows, and checks the
whole `stride * height` extent before use. Pixel order is XRGB8888 bytes in memory (B,G,R,X on
little-endian Linux). `decode_evdev_event` expects exactly the 64-bit Linux UAPI `input_event`
layout (native-endian seconds/useconds, type, code, value); `SYN_REPORT` is consumed as a batch
delimiter. Other event types are preserved verbatim. `open_evdev("/dev/input/eventN")` reads
one record at a time with a caller-supplied timeout capped at one second; disconnects, partial
records, and malformed timestamps are explicit errors. `InputBatch` has 128 records and counts
refused pushes.

## ABI provenance

Layout and constant planning used the locally installed Linux UAPI headers
`/usr/include/linux/input.h`, `/usr/include/linux/vt.h`, `/usr/include/linux/kd.h`,
`/usr/include/drm/drm.h`, `/usr/include/drm/drm_mode.h`, and
`/usr/include/drm/drm_fourcc.h` (kernel DRM UAPI). The evdev decoder follows the 64-bit
`struct input_event`; it is not portable to 32-bit ABI. No DRM ioctl has been exercised here.
The KMS code uses legacy GETRESOURCES/GETCONNECTOR/GETENCODER/GETCRTC, CREATE_DUMB,
MAP_DUMB, ADDFB, SETCRTC, PAGE_FLIP, RMFB, and DESTROY_DUMB UAPI calls. Dumb-buffer layout is
linear and implicit; modifiers and atomic KMS are refused by omission. The upstream source is
the Linux kernel UAPI under `include/uapi/linux/`; hardware behavior must be qualified on the
target GPU/driver and console.
The connected-status value (`1`) follows the kernel's `connector_status_connected` enum in
[`include/drm/drm_connector.h`](https://github.com/torvalds/linux/blob/master/include/drm/drm_connector.h).
The kernel's [DRM KMS documentation](https://docs.kernel.org/gpu/drm-kms.html) describes dumb
buffers, legacy modesetting and page-flip synchronization.

## Validation

The source includes unit tests for frame-bound checks, event decoding, ioctl numbers/layouts, and
explicit device path validation. They were not run in this delegated change; the coordinating
agent owns builds, tests, formatting, and workspace integration. This crate has not been tested
on hardware.

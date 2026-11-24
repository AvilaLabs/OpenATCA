# oa-display-present

Linux-only direct display/input foundation. This crate has no dependencies and is deliberately
not yet in the product workspace. Its pure surface validates bounded XRGB8888 frame extents and
decodes Linux 64-bit evdev records into fixed-capacity plain input records.

`open_kms` currently refuses with `PresentError::NotActive` before opening a path. It does not
claim to present frames. A complete usable session must first capture the original CRTC state,
verify active VT/session ownership, find a connected connector and compatible encoder/CRTC,
allocate two scanout buffers, modeset, wait for page-flip completion, and restore the original
state on every exit path. Those operations are not implemented in this increment. Do not wire
this refusal path as a working display backend or claim hardware qualification.

`validate_frame` accepts at most 4096 x 2160, 64 MiB, four-byte aligned rows, and checks the
whole `stride * height` extent before use. Pixel order is XRGB8888 bytes in memory (B,G,R,X on
little-endian Linux). `decode_evdev_event` expects exactly the 64-bit Linux UAPI `input_event`
layout (native-endian seconds/useconds, type, code, value); SYN reports delimit batches and are
not returned as actions. Unknown event types and malformed timestamps are explicit errors.
`InputBatch` has 128 records and counts refused pushes.

## ABI provenance

Layout and constant planning used the locally installed Linux UAPI headers
`/usr/include/linux/input.h`, `/usr/include/drm/drm.h`, and
`/usr/include/drm/drm_mode.h` (kernel DRM UAPI). The evdev decoder follows the 64-bit
`struct input_event`; it is not portable to 32-bit ABI. No DRM ioctl has yet been issued by this
crate. The upstream source is the Linux kernel UAPI under `include/uapi/linux/input.h` and
`include/uapi/drm/`; hardware behavior must be qualified on the target GPU/driver and console.

## Validation

The source includes unit tests for frame-bound checks and event decoding. They were not run in
this delegated change; the coordinating agent owns builds, tests, formatting, and workspace
integration. This crate has not been tested on hardware.

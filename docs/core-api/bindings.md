# Provisional C and Python bindings

**Status: Preview proposal; ABI names and layouts are not stable.** This document describes
the hand-written `openatca-core-ffi` C boundary and its explicit-path Python `ctypes` wrapper.
It does not claim independent ABI review or platform qualification. The native library is not
loaded when `openatca` is imported; callers choose an absolute library path with `load()`.

## Shared rules

The Rust implementation calls the existing `oa-*` functions. The C layer adds no geometry,
math, parsing, or integrity algorithms. Functions return fixed-width status codes and write
operation outputs only on success; the error record is updated with the status, while the bounded
message API writes its required-size output on a short-buffer response. `OaErrorInfo` carries the
status, a location kind, and a zero-based argument or element location. Panic containment reports
`INTERNAL_PANIC`; it does not validate
arbitrary caller addresses. Required pointer validity, alignment, extent, writable access, and
non-overlap remain documented caller preconditions.

Python uses `ctypes` with fixed signatures and translates nonzero statuses into `OpenAtcaError`.
Python owns its input buffers for the duration of each call. It converts returned values to
immutable result dataclasses or scalar tuples. Importing the package neither searches for nor
loads a native library.

## L0 function inventory

| Rust source | C surface | Python surface | Bounds / ownership |
| --- | --- | --- | --- |
| `oa-ieee` | `oa_math_f64_unary` operation codes for `sqrt`, `floor`, `ceil`, `trunc`, `round` | `ScalarMath.sqrt/floor/ceil/trunc/round` | Scalar values; selected operation code only |
| `oa-math` | `oa_math_f64_unary`, `binary`, `ternary`, `powi`, `sin_cos`, `compare`, `is_sign_negative` | `ScalarMath` named scalar methods | No retained memory; invalid operation is `BAD_OPERATION` |
| `oa-math` f32 helpers | `oa_math_f32_unary`, `binary`, `ternary`, `compare`, `is_sign_negative` | `ScalarMath` f32 helper methods | Scalar inputs and outputs; Rust helper semantics preserved |
| `oa-sha256` | `oa_sha256_digest`, `oa_sha256_hex` | `Integrity.sha256/sha256_hex` | One-shot input capped at `MAX_MESSAGE_BYTES`; caller owns input and fixed output buffers |
| `oa-geo` WGS-84 geodesic | `oa_geo_wgs84_inverse`, `oa_geo_wgs84_direct` | `Geodesy.inverse/direct` | Stateless; angles in degrees, distances in metres |
| `oa-geo` rhumb | `oa_geo_wgs84_rhumb_inverse/direct`, `oa_geo_wgs84_rhumb_point_at_fraction` | `Geodesy.rhumb_*` | Stateless; fraction is validated by core in `[0,1]`; no invented fallback |
| `oa-geo` separation | `oa_geo_wgs84_geodesic_rhumb_separation` | `Geodesy.geodesic_rhumb_separation` | Sample count is core-bounded to `2..=100000`; `at_fraction` is a maximizing fraction, and any finite fraction in `[0,1]` is valid when the maximum has tied locations (including identically zero separation) |
| `oa-geo::Polygon` | `oa_geo_polygon_new_wgs84`, `contains_wgs84`, `edge_count`, `free` | `Polygon` context manager | Constructor borrows coordinate arrays only for the call; opaque immutable handle owns prepared polygon; at most 100001 input vertices; free exactly once |

The polygon handle is the only retained native allocation in this slice. Python's context
manager releases it with `oa_geo_polygon_free`; callers must not use a handle after closing it.
The C ABI cannot make double-free, use-after-free, concurrent free/use, or forged pointers safe.

## Status and error surface

Codes 0–11 are provisional and appear in `include/openatca.h` and the Python wrapper. They cover
successful calls, invalid arguments, non-finite values, coordinate ranges, convergence, short
message buffers, contained panic, invalid core parameters, excessive lengths, unknown operation
codes, and bounded-allocation failure. Message text is queried through the existing size-query
and all-or-nothing `oa_core_error_message` interface. Python exposes `OpenAtcaError.code`,
`location_kind`, `location`, and the optional native message.

## Evidence and remaining work

The C source runner and Python source runner use the same committed synthetic scalar, digest,
rhumb, and polygon vectors. Existing inverse/direct vectors remain unchanged. Source presence and
shared vector definitions are not evidence that a native library was built, tests passed, ABI
layout was independently reviewed, or every supported platform was qualified. Those checks are
queued for the coordinator after source review; the ABI remains Preview until the §8 evidence and
approval conditions are met. No Stable designation is made here.

## Planned module expansion

The §10 inventory also requires container, XML/JSON/CSV, CAT021/CAT048/CAT062, CIFP, NASR,
FNS-AIXM/NOTAM, and bounded GRIB2 APIs with Python wrappers. Each parser API must disclose its
accepted edition/profile and explicit byte, record, nesting, and work bounds before it is exposed.
Those modules are staged separately; this first source slice does not imply those bindings exist.

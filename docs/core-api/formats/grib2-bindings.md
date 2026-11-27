# GRIB2 C and Python bindings

**Status: Preview API proposal.** This is a hand-written binding to the existing `oa-grib2`
decoder. It is not a general GRIB2 parser, forecast product, or stable ABI. The Python package
loads no native library until the caller supplies a path to `load()`.

## Accepted decoder profile

One call accepts exactly one complete GRIB Edition 2 message. The binding delegates to the existing
`oa-grib2::inspect_stream` and `decode_message` implementations. It supports grid templates 3.0
(latitude/longitude) and 3.30 (Lambert conformal), product templates 4.0 and 4.8, packing templates
5.0, 5.2, 5.3 and 5.40, and explicit/reused bitmaps. Other editions, templates, malformed framing,
unsupported packing, and invalid fields return nonzero statuses. Structural inspection does not
promise that a packing template can be decoded; only a successful fill means values were decoded.

The adapter preserves the source discipline/category/number, named parameter, product/packing and
grid template numbers, level kind/value, forecast unit/value, reference and interval-end times,
grid scan/flags and the supported template parameters. For grid 3.0, the four leading grid values
are first latitude/longitude and increments in degrees, with row direction/global coverage as
separate flags. For grid 3.30, the same four slots carry first latitude/longitude followed by zeros;
Lambert parameters occupy the named Lambert fields and use the decoder's units. Fields are ordered
as in the message; each has a flat `value_offset` and `value_count`. A bitmap-missing point is IEEE
NaN in the values array and is also included in `missing_count`. No absent value is replaced by 0.

Named-parameter codes are `0=other`, `1=TMP`, `2=RH`, `3=UGRD`, `4=VGRD`, `5=VVEL`, `6=HGT`,
`7=PRES`; the raw discipline/category/number triple remains present for every field. The caller
should use that raw triple when the named code is `other`.

## Bounds and memory

The ABI uses the existing decoder limits without raising or narrowing them: 256 MiB per message,
16 Mi points per field, 16 fields per message, 64 Mi total points per message, and the decoder's
4 Mi + 64 points per input byte work budget. JPEG 2000 retains `oa-j2k`'s 72 MiB scratch ceiling.
The complex-packing decoder also bounds field points and group count at 16 Mi each. At those
source limits, its reference and width tables are at most 64 MiB each, its `usize` group-length
table at most 128 MiB on the supported 64-bit ABI, its `i64` point table at most 128 MiB, and the
field's final `f32` values at most 64 MiB: 448 MiB of logical vector payload at the declared
maxima. Bitmap expansion can add another 64 MiB while the packed `f32` vector remains live. Up to
48 Mi previously decoded field points can contribute another 192 MiB at that instant because the
whole message total is capped at 64 Mi points. Thus the conservative source-derived peak for the
complex-packing vector payloads is 704 MiB, plus bitmap bits, up to 16 small `FieldInfo`/`Field`
records, allocator bookkeeping and allocator size-class rounding. The JPEG 2000 path has a separate
72 MiB decoder scratch cap; it cannot overlap the complex-packing tables because one field has one
packing template. The binding itself adds at most 16 inspection records and a 16-record stack
descriptor array, and it does not allocate a second Rust values vector. These figures derive from
the existing field/group/message caps; they are accounting bounds, not newly imposed caps.

The C ABI borrows input and writes only caller-owned output buffers. A maximal C call may therefore
also have 256 MiB of caller value capacity live beside the decoder's bounded allocations. Python
keeps the caller's `bytes`, copies up to 256 MiB for the native call, and allocates a compact
256 MiB `array('f')` at the message point cap; peak process memory must include those buffers as
well as the native decoder's bounded allocations. The binding imposes no smaller profile cap.

These are explicit algorithmic bounds, not a promise that a 256 MiB call fits every process memory
limit. No hidden persistent state or retained input pointer is created.

## C interface and failure behavior

`oa_grib2_inspect_message` validates structural framing and returns required field/value counts.
It does not decode values or reject a structurally valid unsupported packing method. The caller
then calls `oa_grib2_decode_message` with capacities from inspection. If either capacity is short,
`BUFFER_TOO_SMALL` returns both required counts and leaves both arrays unchanged. Successful decode
writes complete descriptors and values only after the existing decoder has completed. Other
failures leave output arrays and required-count outputs unchanged. Null, misaligned, undersized, or
overlapping C addresses cannot be validated by the ABI; their validity and disjointness are caller
preconditions. Statuses 80–82 are GRIB2 format, unsupported-profile, and existing-limit refusals.

Python exposes `load(path).grib2.inspect_message(bytes)` and
`load(path).grib2.decode_message(bytes)`. The latter performs query then fill, verifies the native
counts remain consistent, and returns `Grib2Message(fields, values)`; fields are immutable metadata
records and `values` is a compact mutable `array('f')` owned by that result. Native errors become
`OpenAtcaError` with the matching status.

The committed synthetic `grib2-bitmap-v1.tsv` vector contains a 4×3 regular grid, one UGRD field,
reference/forecast metadata, and seven present plus five missing values. Rust, C and ctypes
regressions exercise its metadata, NaN bitmap preservation, truncation, unsupported packing, and
capacity atomicity. It is schema-generated synthetic input, not SWIM data. These source changes and
vectors do not establish an independent ABI review, platform qualification, weather freshness, or
operational use. HZ-WEATHER-001 concerns the downstream weather/pressure-altitude consumers; this
standalone decoder does not mitigate that consumer hazard. The §8 Stable bar remains unmet.

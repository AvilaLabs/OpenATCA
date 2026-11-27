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
| `oa-grib2` exact-subset decoder | `oa_grib2_inspect_message`, `oa_grib2_decode_message` | `Grib2Bindings.inspect_message/decode_message` | One complete borrowed message; existing templates and 256 MiB / 16-field / 64 Mi-point/work bounds; typed metadata and `f32` values; bitmap missing is NaN; Preview only, see [profile](formats/grib2-bindings.md) |
| `oa-geo` WGS-84 geodesic | `oa_geo_wgs84_inverse`, `oa_geo_wgs84_direct` | `Geodesy.inverse/direct` | Stateless; angles in degrees, distances in metres |
| `oa-geo` rhumb | `oa_geo_wgs84_rhumb_inverse/direct`, `oa_geo_wgs84_rhumb_point_at_fraction` | `Geodesy.rhumb_*` | Stateless; fraction is validated by core in `[0,1]`; no invented fallback |
| `oa-geo` separation | `oa_geo_wgs84_geodesic_rhumb_separation` | `Geodesy.geodesic_rhumb_separation` | Sample count is core-bounded to `2..=100000`; `at_fraction` is a maximizing fraction, and any finite fraction in `[0,1]` is valid when the maximum has tied locations (including identically zero separation) |
| `oa-geo::Polygon` | `oa_geo_polygon_new_wgs84`, `contains_wgs84`, `edge_count`, `free` | `Polygon` context manager | Constructor borrows coordinate arrays only for the call; opaque immutable handle owns prepared polygon; at most 100001 input vertices; free exactly once |
| `oa-json` | `oa_json_parse` | `TextBindings.parse_json` | One-shot default parser limits; flat preorder node table and decoded string/exact lexical-number pool; capacity query then all-or-nothing fill; 256 MiB Python output ceiling |
| `oa-xml` | `oa_xml_parse`, `oa_xml_decode_span` | `TextBindings.parse_xml`, `decode_xml_span` | One-shot default parser limits; spans reference immutable input; namespace URIs copied to a pool; DTD refused; exact parser offsets preserved |
| `oa-csv` | `oa_csv_parse` | `TextBindings.parse_csv` | One-shot default reader limits; header is row zero; quoted fields decoded into a byte pool; physical starting line retained; input capped at 32 MiB by the adapter |
| `oa-asterix` CAT021/048/062 | `oa_asterix_decode_block` | `AsterixBindings.decode_block` | One bounded block; explicit CAT021 Ed 2.4/2.7 selector; caller-owned fixed rows, two-call capacity protocol, exact typed/raw/unprojected masks and raw source units; see [ASTERIX binding preview](formats/asterix-bindings.md) |
| `oa-zip` | `oa_zip_list`, `oa_zip_extract_entry` | `Archives.parse_zip`, `Archives.extract_zip_entry` | Borrowed ZIP bytes; caller-owned flat metadata/name/output buffers; existing oa-zip parser and extraction limits; no filesystem extraction |

The polygon handle is the only retained native allocation in this slice. Python's context
manager releases it with `oa_geo_polygon_free`; callers must not use a handle after closing it.
The C ABI cannot make double-free, use-after-free, concurrent free/use, or forged pointers safe.

## Status and error surface

Codes 0–11 are provisional and appear in `include/openatca.h` and the Python wrapper. They cover
successful calls, invalid arguments, non-finite values, coordinate ranges, convergence, short
message buffers, contained panic, invalid core parameters, excessive lengths, unknown operation
codes, and bounded-allocation failure. Message text is queried through the existing size-query
and all-or-nothing `oa_core_error_message` interface. Python exposes `OpenAtcaError.code`,
`location_kind`, `location`, and the optional native message. The existing DEFLATE statuses occupy
13–17, and tar statuses 18–22. ASTERIX returns the shared generic status (for example,
`PARSE_ERROR`) while `OaAsterixError.kind` carries the detailed decoder classification 1–23; status
values 23–31 are currently unassigned. ZIP statuses occupy 32–40 and GRIB2 statuses 48–50. These assignments remain
provisional and are mirrored by the Python status-name map.

## Evidence and remaining work

The C and Python runners use shared committed synthetic scalar, digest, rhumb, polygon, text,
container and ZIP vectors. Existing inverse/direct vectors remain unchanged. Source presence and
shared vector definitions are not evidence that a native library was built, tests passed, ABI
layout was independently reviewed, or every supported platform was qualified. Those checks are
queued for the coordinator after source review; the ABI remains Preview until the §8 evidence and
approval conditions are met. No Stable designation is made here.

## Planned module expansion

The remaining §10 modules are staged separately. The exact-subset GRIB2 C/ctypes binding is a Preview
source slice described in [GRIB2 bindings](formats/grib2-bindings.md); its accepted templates,
bounds, failure atomicity, and synthetic vectors do not claim general GRIB2 support or Stable status.

### Proposed tar metadata binding (Preview; awaiting designated API review)

Candidate C surface: `oa_tar_parse(input, input_length, max_entries, entries, entry_capacity,
entry_count, name_pool, name_pool_capacity, name_pool_length, error)` accepts a borrowed tar byte
slice and returns metadata only: ordered `OaTarEntry` records plus one UTF-8 byte-name pool.
The 48-byte record contains `name_offset`, `name_length`, effective `size`, `mtime`,
`header_offset`, `typeflag`, and seven zeroed reserved bytes. Each record preserves the `oa-tar::Header`
name bytes, effective payload size, reported typeflag, mtime, and `Header::offset` (the first
extension record's offset when extension records precede the member, otherwise its header offset).
Duplicate names
and unknown typeflags remain distinct; a stored NUL typeflag follows `oa-tar` normalization to
`'0'`. The parser does not return payload bytes, apply a filesystem path policy, or extract files.

The proposed one-shot limits are 64 MiB input and a caller-selected entry cap no greater than
4,096, with `oa-tar`'s `DEFAULT_LIMITS` for member size, name bytes, extension-record size/count,
and total extension bytes. Caller capacities use an all-or-nothing size query: if either output
capacity is short, `BUFFER_TOO_SMALL` reports exact entry and name-pool sizes. Malformed headers,
checksum failures, truncated members/end markers, and limit refusals leave caller arrays and size
metadata unchanged. Python candidate: `Archives.parse_tar(bytes, max_entries=4096)` returns an
immutable `TarResult.entries` tuple of `TarEntry` records. These names and layouts are
proposals, not compatibility promises; designated API review remains required before freezing.
ASTERIX is implemented as a Preview source slice. The remaining modules are staged separately;
this implementation does not establish independent ABI review, platform qualification, or the
Stable evidence bar.

### Proposed ZIP metadata and selected-entry binding (Preview; awaiting designated API review)

Candidate C functions are `oa_zip_list(input, input_length, entries, entry_capacity,
entry_count, name_pool, name_pool_capacity, name_pool_length, error)` and
`oa_zip_extract_entry(input, input_length, entry_index, output, output_capacity, output_length,
error)`. Both borrow archive bytes for the call and retain no pointers or handles. Listing exposes
ordered central-directory metadata only. `OaZipEntry` is a 56-byte fixed-width record with five
64-bit name/size/offset fields, CRC-32, source flags and method, ZIP64 and bit-11 indicators, and
six named zero-padding bytes. Name spans index UTF-8 bytes returned by `oa-zip`; they are subject
to that decoder's UTF-8/ASCII name profile, not a lossless copy of arbitrary legacy bytes.
Encryption and unsupported methods refuse the complete archive rather than being silently
skipped.

`oa_zip_list` validates the complete central directory under `oa-zip::DEFAULT_LIMITS`. If either
metadata capacity is short it returns exact entry and name-pool sizes with no array writes.
Selected extraction is index-based and writes only to caller memory. Its zero-capacity query
validates directory metadata and reports the declared output size; local-header, stream, size, and
CRC integrity are established only by successful fill. A short output buffer reports required
size but remains unchanged. Other failures leave output bytes and size metadata unchanged. No
directory or filesystem API is provided.

The binding accepts at most 1 GiB of borrowed archive input and retains existing bounds: 10,000
entries, 4,096-byte names, 256 MiB per entry, 1 GiB total declared output, ratio 200 except the
1 MiB small-entry exemption, and the existing EOCD search bound. Candidate Python methods return
immutable `ZipResult.entries` / `ZipEntry` values and extracted `bytes`; the native library remains
an explicit caller-supplied path. The shared `zip-v1.tsv` vectors are synthetic CC0 inputs covering
stored/deflated entries, UTF-8 flag behavior, truncated/unsupported/corrupt/unsafe/ratio refusals,
capacities, and CRC checks across Rust, C, and ctypes. The names and layout remain proposals until
designated API review. No Stable designation is made; §8 evidence remains outstanding.


### Proposed GRIB2 message binding (Preview; awaiting designated API review)

See [the GRIB2 binding contract](formats/grib2-bindings.md) for the precise `inspect_message` and
`decode_message` C/ctypes surface, fixed record layout, exact delegated decoder profile, buffer
query behavior, bounds, and limitations. The shared synthetic bitmap vector is exercised by Rust, C,
and Python source tests; no publication or stable ABI claim is made.


### Proposed GRIB2 message binding (Preview; awaiting designated API review)

See [the GRIB2 binding contract](formats/grib2-bindings.md) for the precise `inspect_message` and
`decode_message` surface, fixed record layout, delegated decoder profile, bounds, failure atomicity,
and limitations. The synthetic bitmap vector is exercised by Rust, C, and ctypes source tests; no
publication or Stable designation is made. Relevant existing hazard records: OA-GRIB-001 through
OA-GRIB-022.

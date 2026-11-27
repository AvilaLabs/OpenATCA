# Provisional C and ctypes FAA CIFP binding

**Tier:** Preview. This surface binds only the repository's existing `oa-arinc424` FAA
`FAACIFP18` / ARINC 424-18 profile. It is not a general ARINC 424 parser and does not add a
second decoder. Names and layout remain under designated API review; nothing here is a release or
Stable claim.

## Decoder profile and caps

The adapter calls `oa_arinc424::parse_bytes` and projects its typed `Cifp` result. Accepted input
is one complete FAA CIFP byte sequence, including HDR01..HDR05 and 132-character records. It
retains the decoder's section support/skipping, cycle rule, continuation grouping and per-group
limits. Unsupported section/subsection records are not retained individually; only their aggregate
section counts are available, so the binding does not report them as decoded typed rows. Soft row
errors and `errors_dropped` remain distinct. The header CRC is
reported as text with `crc_verified = false`, matching the decoder's documented behavior.

Binding bounds are checked before parsing: 128 MiB input, at most 600,000 physical lines,
source label at most 256 UTF-8 bytes, and no single line over `oa_arinc424::MAX_LINE_LEN`. These
bounds admit about 397,000 132-byte records plus line terminators with headroom. The decoder's
2,000,000 record and all per-group limits are not changed; the adapter's 600,000 physical-line
ceiling is an explicit binding resource ceiling. The parser is bounded by input bytes, line length,
physical-record count, and its existing continuation, airway, route, leg, boundary, error, and
header limits. After parse, projection planning computes exact fixed-row bytes and exact UTF-8 arena
bytes with checked arithmetic and rejects output above 768 MiB before allocating the flat projection.
This cap includes separate route/leg, airway-fix, boundary, continuation, metadata, count and error
rows, so it bounds the duplicate flat representation live beside the decoded graph. It does not
claim that all parser and allocator overhead has been measured. The
projection uses checked `try_reserve` calls and writes spans directly; no JSON or full cloned CIFP
graph is created. The exact row/vector count formulas derive from the decoder's collection lengths,
which cannot exceed physical input lines or the decoder's named per-group limits. CIFP C and
ctypes vectors exercise valid, malformed, and capacity-refusal cases; the 128 MiB/600,000-line
ceiling is enforced in code but its upper boundary is not allocated by these synthetic tests.

## Output model

All C numeric fields use fixed-width integer types; all string references are `(u64 offset,
u64 length)` into one UTF-8 arena. `UINT64_MAX` is reserved for absent offsets and is emitted by an
explicit branch, not a native `usize` cast. No pointers or Rust ownership cross the call. Rust's
safe projection module defines pointer-free fixed-layout row structs and computes checked row and
arena sizes before allocating the projected `CifpFlat`; only `ffi.rs` interprets C pointers.

The provisional output tables are:

- `OaCifpHeader` (one row), `OaCifpHeaderText` (the raw HDR records in order), inline metadata on
  each retained typed record (1-based source line, record cycle, customer area and file record
  number spans), `OaCifpSectionCount`, and `OaCifpCycleCount`.
- Fixed-layout tables for `Airport`, `Runway`, `Localizer`, `VhfNavaid`, `Ndb` (with an explicit
  terminal-NDB flag), `Waypoint`, `Airway`, `AirwayFix`, `Procedure`, `Route`, `Leg`, `Airspace`,
  `BoundaryPoint`, and `GridMora`. Source metadata is inline on records that retain it; grouped
  airway/procedure rows carry their documented first-line field. Option fields have explicit presence bits; absent is never
  encoded as a valid zero. Raw enumerated unknown characters remain present in their typed code
  fields.
- `OaCifpContinuation` records carry owner kind/index, 1-based source line, continuation number,
  application type and text span. This covers both procedure-leg and airspace continuation rows.
  Unsupported section/subsection records are not retained individually by the current decoder;
  only its aggregate section counts are available. No per-record `Other` row or provenance is
  claimed by this binding. Likewise, grouped airspace boundary points carry `UINT64_MAX` for
  source line because the typed `Airspace` model does not retain each point's physical line.
- `OaCifpError` preserves source line and a stable error-kind code plus field columns, observed
  value/reason spans, and numeric limit/count values when applicable. `errors_dropped` is a
  separate count; an error cap is not represented as zero errors.

Procedure route and leg arrays use explicit `(start,count)` ranges. Airway fixes use the same
range pattern. Airspace boundary points and continuations use ranges. These rows are flattened in
source/group order; no procedure, fix, or continuation is serialized as opaque JSON. Typed
altitudes retain kind (`Feet`, `FlightLevel`, `Ground`, `Unlimited`, `Unknown`) and raw value;
positions retain decimal-degree `f64` values only where `oa-arinc424` already decoded them. The
header cycle and each record cycle are separate fields. The adapter never infers a date or
calendar timestamp from a CIFP cycle.

`OaCifpCounts` carries required counts for every table and the UTF-8 arena. A call first
validates the complete input and builds the safe decoder result/plan. A zero-capacity query returns
`BUFFER_TOO_SMALL` with exact required counts and writes no arrays. A fill with any short table or
arena also writes no arrays. Parse refusal leaves counts and all output buffers unchanged except
the typed `OaCifpError`; successful fill writes all tables and then counts. Every caller buffer
must be valid, aligned, writable and pairwise non-overlapping with input, counts and error. These
remain C caller preconditions, as for the existing FFI.

The detailed `OaCifpError.kind` is a pinned enum mapping of the current `oa_arinc424::ErrorKind`
variant; `status` uses shared `PARSE_ERROR`, `BUFFER_TOO_SMALL`, `INVALID_ARGUMENT`,
`LENGTH_TOO_LARGE`, `ALLOCATION_FAILED`, or `INTERNAL_PANIC`. Source and line remain available on
record-level errors and fatal parse errors. The current synthetic malformed-header vector checks
that error status and nonzero kind survive the C and ctypes boundaries; exhaustive mapping of
every decoder error kind is not claimed.

## C and Python entrypoints

The C function is `oa_cifp_parse(input, input_length, source, source_length, outputs,
counts, parse_error, error)`. `outputs` contains one pointer/capacity pair for each table and the string arena;
`counts` is updated only for success or the explicit capacity query. A null/zero capacity is only
valid for that table when its required count is zero or when the complete call is a query. Shared
status codes stay unchanged.

Python exposes `CifpBindings.parse(bytes, *, source="<bytes>")` through the explicit-path
`load(path).cifp` namespace. `CifpResult` and its nested immutable record values expose named
fields, recursively decoded positions/altitudes, explicit presence masks, and UTF-8 values paired
with their byte offsets and lengths. Native ctypes structures stay private to the wrapper. The
wrapper performs query then fill and checks the same input/output ceilings. Importing Python never
searches for or loads a library.

The public frozen row dataclasses have stable `Cifp*` names and field annotations; `CifpResult`
exposes a named tuple for each row family and `CifpRow` is their union. `CifpError` preserves the
generic status in `code`/`status` and exposes CIFP-specific diagnostic code, kind, source location,
columns, limit/count fields, and flags separately. The parser does not expose live ctypes views.

## Required synthetic vectors

Rust projection, C, and ctypes use the same fabricated 132-byte records. The corpus exercises the
header, airport, runway, localizer, VHF and NDB (including terminal NDB), enroute and terminal
waypoint, MORA, controlled airspace and boundary, airway and airway fix, SID/STAR/approach,
procedure/route/leg linkage, continuation, and level-of-service output families. It also checks
UTF-8 arena spans, a missing-header refusal, and all-or-nothing short-capacity refusal. It does not
cover record-level soft errors or upper resource boundaries. The C11 layout test asserts every declared
CIFP row size and field offset; Rust has matching `repr(C)` layout assertions, and Python checks its
buffer/count/error layouts. Synthetic vectors only; no FAA data file, recordings or private flight
rows. These vectors do not establish Stable evidence or FAA acceptance.

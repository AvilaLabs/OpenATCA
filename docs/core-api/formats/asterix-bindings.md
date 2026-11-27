# Provisional C and ctypes ASTERIX decoder binding

**Tier:** Preview. Names/layouts below are proposals for review, not stable ABI. The binding
delegates decoding to `oa-asterix`; it adds no ASTERIX parsing or aviation interpretation.

## Accepted profiles and input

The first slice accepts one bounded ASTERIX datagram/block at a time for the existing profiles:
Part 1 Ed 3.1 framing, CAT021 Ed 2.4 or Ed 2.7 (caller selects explicitly), CAT048 Ed 1.32, and
CAT062 Ed 1.21. CAT034 and every other category are unsupported. CAT021 has no implicit default:
selector 24 or 27 is required for CAT021, and selector 0 is the not-applicable sentinel for every
non-CAT021 category. Unsupported categories with selector 0 reach the existing decoder and are
reported as unsupported-category decode refusals. The caller borrows the input bytes for
the call; no native handle or input memory is retained. The decoder uses `decode_block_with`,
returns the exact consumed block length even when the supplied slice has a trailing next-block
prefix, and
reports block-relative offsets as defined by `AsterixError`.

The caller supplies a fixed-layout record array. A zero-capacity/null-array call is a size query:
it validates the block and returns the exact required record count and consumed length with
`BUFFER_TOO_SMALL` when records are present. If capacity is short, no record is written. On a
decode refusal, no record or count output is changed; the detailed error output is updated.
Successful calls write all records and then publish counts. Output arrays, count metadata, error
metadata, and input must be pairwise non-overlapping, with pointer validity, alignment and extent
remaining caller preconditions. The input bound is `oa_asterix::MAX_BLOCK_LEN` (65,535 bytes);
record count is bounded by the decoder's `MAX_RECORDS_PER_BLOCK`.

## Fixed projection

`OaAsterixRecord` is a `repr(C)` scalar-only row with this proposed field order and C types:

```c
typedef struct {
  uint8_t category, cat021_edition, source_sac, source_sic;
  uint32_t reserved0;
  uint64_t present_frn_mask, raw_untyped_frn_mask, typed_unprojected_frn_mask;
  uint64_t present_field_mask;
  uint32_t target_address;
  uint8_t target_ident[8];
  uint16_t track_number, time_item;
  uint32_t time_raw;
  uint8_t position_count, velocity_kind, velocity_flags, mode3a_flags;
  struct {
    uint16_t item;
    uint8_t kind, scale;
    int32_t a_raw, b_raw;
  } positions[2];
  int32_t velocity_a_raw, velocity_b_raw;
  uint16_t mode3a_code;
  uint16_t reserved3;
  uint32_t raw_untyped_subitem_mask;
  uint32_t reserved4, reserved5;
  uint8_t vertical_count, reserved6[7];
  struct {
    uint16_t item;
    uint8_t subitem, kind, scale, flags, auxiliary, reserved;
    int32_t raw;
  } vertical[6];
} OaAsterixRecord;
```

`present_field_mask` bits are fixed as: bit 0 source identifier, 1 target address, 2 target
identification, 3 track number, 4 selected time, 5 position slot 0, 6 position slot 1, 7 velocity,
8 Mode 3/A, and 9 one or more altitude/vertical values. No other bit is set in this version.
`time_item` is I021/071, I048/140 or
I062/070; CAT021 I021/072 and reception/transmission times are not projected and are reported in
`typed_unprojected_frn_mask`.

Altitude/vertical `kind` values are 1 geometric height, 2 flight level/barometric altitude, 3
selected altitude, 4 final-state selected altitude, 5 barometric vertical rate and 6 geometric
vertical rate. Scale values are 1 for 6.25 ft, 2 for 0.25 flight level, 3 for 25 ft, and 4 for
6.25 ft/min; `kind` supplies the dimension and each slot's ASTERIX `item`/`subitem` supplies its
meaning. Scale values refer to `CAT021_GEOM_HEIGHT_FT`, `CAT021_FL`/`CAT048_FL`/`CAT062_FL`,
`CAT021_SELECTED_ALT_FT`/`CAT062_SELECTED_ALT_FT`, and `CAT021_VRATE_FPM`/`CAT062_ROC_FPM` in
`oa-asterix::units`. Zero flags means the typed source supplies no relevant validity flag, not
that the value has been independently validated. `velocity_flags` bit 0 preserves CAT021 I021/160
range-exceeded. Vertical `flags` bit 0 preserves CAT021 rate range-exceeded, bits 1–2 CAT048
flight-level not-validated/garbled, bit 3 CAT062 barometric QNH, bit 4 selected-altitude
source-provided, and bits 5–7 CAT021 final-state manage-vertical/altitude-hold/approach.
`auxiliary` carries the selected-altitude source code. These flags are source encoding only, not
independent validity or quality judgments.

Position `kind` values are 1 geodetic, 2 CAT048 polar (range then azimuth), and 3 Cartesian.
Geodetic scale codes are 1 CAT021 I021/130 (2^23 denominator), 2 CAT021 I021/131 (2^30), and 3
CAT062 I062/105 (2^25); polar scale code 4 means range 1/256 NM and azimuth 360/2^16 degrees;
Cartesian scale code 5 means CAT048 I048/042 at 1/128 NM, and 6 means CAT062 I062/100 at 0.5 m.
Velocity `kind` 1 is CAT021 I021/160 (ground speed and track angle raw counts); kind 2 is CAT062
I062/185 (Vx/Vy at 0.25 m/s). These provisional enum values must be pinned in the C header and
Python wrapper tests.

The explicit reserved fields make the layout deterministic: size 192 bytes, alignment 8 bytes.
C compile-time and Rust unit assertions must pin each offset: record identity fields 0–3,
reserved0 4, the four masks 8/16/24/32, address 40, identification 44, track/time-item 52/54,
time raw 56, count/kind/flag bytes 60–63, positions 64 (12-byte stride), velocity 88/92, Mode 3/A
code 96, reserved3 98, raw-subitem mask 100, reserved4/5 104/108, vertical count 112, reserved6
113–119, vertical array 120 (12-byte stride). Nested position fields are at offsets 0/2/3/4/8;
nested vertical fields at 0/2/3/4/5/6/7/8. All padding is named reserved storage.
`cat021_edition` is
0 when not applicable, 24 for Ed 2.4 and 27 for Ed 2.7. `present_frn_mask` marks every FRN the
decoded record contains (`bit FRN-1`); `present_field_mask` marks each projected output field, so
zero values remain distinct from absence. The raw-untyped mask contains only present items in the
existing Rust `record.raw` vector. The typed-unprojected mask contains present typed FRNs whose
decoded values are omitted by this projection (including unexposed members of a compound item).
Neither omission mask is evidence that the wire item was decoded into an output field.

Projected fields are source/SAC-SIC, target address, eight-byte identification, track number,
one selected category-defined time item, up to two category-defined position representations,
one category-defined velocity representation, Mode 3/A, and altitude/vertical values when present.
CAT021 position slots are
ordered I021/130 then I021/131; CAT048 and CAT062 slots are ordered geographic/polar then Cartesian.
The two position slots
preserve both CAT021 I021/130 and /131 or both CAT048/CAT062 position items if the input carries
both. Each slot's `item`, `kind`, and `scale` identify the source item, representation and source
resolution; the position field bits identify which slot is present. `time_item` disambiguates the
selected time source; other typed time items are explicitly marked in the typed-unprojected FRN
mask. `velocity_kind` selects source meaning; `mode3a_flags` contains V/G/L-or-change only where
the existing typed record supplies those flags (CAT021 exposes only the code, so its flags remain
zero). Up to six item-identified altitude/vertical values retain typed raw integers, source flags
and scales. They cover CAT021 I021/140, /145, /146, /148, /155 and /157; CAT048 I048/090; and
CAT062 I062/130, /135, /136, /220 and I062/380 subitem 6. `kind` identifies geometric height,
flight level, selected/final selected altitude, barometric/geometric vertical rate or
calculated/measured altitude; `scale` identifies the source unit. Flags preserve only source flags:
CAT021 rate range-exceeded, CAT048 flight-level not-validated/garbled, CAT062 barometric QNH, and
selected-altitude source-provided; `auxiliary` carries the selected-altitude source code. These
flags are source encoding only, not independent validity or quality judgments. Fields not
applicable to a category remain zero and their `present_field_mask` bit is clear. No numeric
output is converted to float or synthesized across categories. All numeric
aviation quantities remain source raw counts; item scales are documented from the existing
`oa-asterix::units` constants.

Position kinds name the source item and preserve its native representation: CAT021 low-resolution
I021/130 and high-resolution I021/131 geographic counts, CAT048 polar I048/040 or Cartesian
I048/042 counts, and CAT062 geographic I062/105 or Cartesian I062/100 counts. Velocity kinds
preserve CAT021 I021/160 speed/track counts or CAT062 I062/185 Cartesian counts. Time raw counts
are day-relative 1/128-second units for CAT021 I021/071, CAT048 I048/140 and CAT062 I062/070;
they do not identify a calendar date or resolve midnight rollover. Target identifiers and
addresses are exposed only where the existing typed decoder supplies them (CAT021 I021/170 and
/080, CAT048 I048/240 and /220, CAT062 I062/380 subitems 2 and 1). No fields are synthesized
across categories.

The `raw_untyped_frn_mask` reports items that `oa-asterix` retained as raw because no typed
decoder exists. `typed_unprojected_frn_mask` reports present typed FRNs whose decoded values are
omitted from this C projection. These masks make omissions visible; they do not claim the item is
unsupported by ASTERIX or invalid. `raw_untyped_subitem_mask` is limited to CAT062 I062/380
subitems held in `AircraftDerived.other` as raw bytes; bit `subitem-1` means that subitem was
present but not typed or projected. It is zero for CAT021 and CAT048. This prevents raw I062/380
subitems from being mislabeled as typed output. FRN bit `n-1` denotes FRN `n`; current profiles
fit in 64 bits. The Rust projection must derive these masks from the decoded record and its raw
items, not from input claims or a second parser.

## Proposed function/error surface

`oa_asterix_decode_block(input, input_length, cat021_edition, records, record_capacity,
counts, parse_error, error)` returns existing fixed status codes. `OaAsterixCounts` contains `category`,
`edition`, `consumed_bytes`, `required_record_count`, and `written_record_count`. Counts are
published on success or `BUFFER_TOO_SMALL`; a short array capacity writes no record. On other
failures, counts and all record bytes remain untouched. `OaAsterixError` contains `status`,
`category`, zero-based `record`, ASTERIX item number, `subitem`, byte `offset`, and a stable
provisional error-kind code mapped from `AsterixError`; it is updated only for an error, not on
success. The status distinguishes success, short capacity, invalid arguments, oversize input,
decode refusal, allocation failure and contained panic. `NO_RECORD`, `NO_ITEM`, and `NO_OFFSET`
keep their Rust sentinel meanings. The operation catches panics at the ABI boundary consistently
with the existing provisional FFI policy.

Proposed `OaAsterixCounts` is 32 bytes/alignment 8 with category/edition at 0/1, reserved fields
at 2–7, and consumed/required/written counts at 8/16/24. Proposed `OaAsterixError` is 32
bytes/alignment 8 with status/kind at 0/4, category/subitem at 8/9, item at 10, record at 12,
reserved bytes at 16–23, and offset at 24. Error `record`, `item`,
`subitem`, and `offset` map directly from `AsterixError`; the 64-bit offset preserves the
`usize::MAX` `NO_OFFSET` sentinel as `UINT64_MAX` by an explicit sentinel branch, never by a native
integer cast, on the supported 64-bit x86-64 and aarch64 targets. The caller passes
both metadata structs; they must not overlap input or record output. Numeric error-kind values
must be exhaustively mapped and pinned in header/ctypes tests.

Python exposes an explicit-path `AsterixBindings.decode_block(bytes, *, cat021_edition=...)` wrapper over
ctypes as `load(path).asterix.decode_block(...)`. It runs the same two-call size-query/fill protocol and returns immutable dataclasses with
raw values, item/profile identifiers and presence/omission masks. Import still does not search for
or load a native library.

## Required evidence for this slice

One synthetic language-neutral vector suite must drive the Rust reference, C ABI and ctypes wrapper
with the same byte inputs. It covers each accepted category, both CAT021 editions, an unconsumed
trailing-block prefix with exact `consumed_bytes`, multiple
records, truncated header/block/record, unsupported category, malformed item, and capacity query
and refusal. Every refusal checks exact error fields and verifies no partial record output; every
success checks the fixed projection against the Rust typed record. No private capture, flight
record, FAA SWIM payload or external dependency is permitted. The ABI remains Preview until the
core library's published evidence and review requirements are separately met.

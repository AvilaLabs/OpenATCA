# `oacv` v1 initial profile

The corpus format uses UTF-8 JSON objects and sibling binary members. The Rust checker rejects
duplicate keys, unknown/missing fields, unsupported values, unsafe paths, incorrect digests and
unlisted files. JSON uses `oa-json` limits configured by the checker. The current profile supports
successful SHA-256 digest, WGS-84 inverse/direct/rhumb inverse, and selected ASTERIX decode cases,
plus typed geodesy and ASTERIX refusals. Encoding and other modules are not implemented in this
checker yet.

`vectors/manifest.json` has exactly `schema`, `license`, and `cases`. `schema` is
`oacv-manifest/1`; `license` is `CC0-1.0`. Each case entry has exactly `id`, `case`, `bytes`, and
`sha256`; the last two bind the UTF-8 case document. IDs are unique lowercase ASCII names. `case`
is a relative path using lowercase ASCII letters, digits, dot, underscore, slash, or hyphen. It
cannot be absolute, contain empty/dot/parent components, or exceed the path/component bounds.

Each case document has exactly `schema`, `id`, `module`, `operation`, `edition`, `limits_profile`,
`input`, `expected`, and `provenance`. `schema` is `oacv-case/1`; `id` equals the manifest ID;
`limits_profile` is `default-v1`. Provenance has exactly `kind`, `license`, `author`, `reference`,
and `method`; the current accepted values are `synthetic-spec-derived` and `CC0-1.0`.

Binary input is `{ "kind":"binary", "file":..., "bytes":..., "sha256":... }`, relative to
the case document directory. Structured input is `{ "kind":"structured", "value":... }`.
Successful expected output is `{ "kind":"success", "value":... }`. A refusal is
`{"kind":"refusal","error":{"code":"...","location":{"kind":"argument_index","value":0}}}`.
The initial geodesy adapter maps `GeoError::NonFinite`, `LatitudeOutOfRange`, and
`LongitudeOutOfRange` to `geo.non_finite`, `geo.latitude_out_of_range`, and
`geo.longitude_out_of_range`, with a zero-based coordinate argument index in operation order:
latitude 1, longitude 1, latitude 2, longitude 2. It does not compare diagnostic prose.
Geodesy input bit strings may encode NaN or infinity for expected refusals; successful cases must
pass the public finite-coordinate constructor.
The rhumb inverse profile uses the same four coordinate input fields, the `rhumb_inverse`
operation, and the same WGS-84 edition. Its success expectation names distance/course decimal
references, absolute tolerances and units. A pole refusal uses the stable `geo.rhumb_pole` code and
the zero-based endpoint argument index (0 or 2).
The direct profile uses `lat1_deg_f64_bits`, `lon1_deg_f64_bits`, `azimuth_deg_f64_bits`, and
`distance_m_f64_bits`. Its success values are destination latitude/longitude and terminal azimuth
decimal references with absolute tolerances. Non-finite azimuth/distance refusals use
`geo.non_finite` and argument indices 2/3.

The SHA-256 profile uses `module="sha256"`, `operation="digest"`,
`edition="FIPS-180-4/SHA-256"`; its expected value is
`{ "sha256_hex": <64 lowercase hex digits> }`. The geodesy profile uses `module="geo"`,
`operation="inverse"`, `edition="WGS84-Karney2013"`; coordinates are binary64 bit strings
named `lat1_deg_f64_bits`, `lon1_deg_f64_bits`, `lat2_deg_f64_bits`, and `lon2_deg_f64_bits`.
Its successful expected value contains `distance_m_reference_decimal`,
`distance_m_absolute_tolerance`, and `distance_m_unit="m"`. Reference and tolerance are decimal
strings so JSON floating-point parsing cannot round them; the tolerance is an absolute error bound
for that named quantity.

The initial ASTERIX adapter supports `module="asterix"`, `operation="decode"`, CAT021 Ed 2.4 and
2.7, CAT048 Ed 1.32, and CAT062 Ed 1.21, with an exact binary block as input. A success value names the
`projection` schema, category, record count and ordered records. The current `cat021-minimal-v1`,
`cat048-minimal-v1`, and `cat062-minimal-v1` projections compare only the fields listed in each case's
`expected.value.records` object; they must not be described as full-record conformance. A refusal
carries a stable `asterix.*` code, category, nullable zero-based record index and item number
(`null` represents the public `NO_RECORD` or `NO_ITEM` sentinel), subitem number and zero-based
byte offset. Diagnostic prose is not compared.

Limits: 1 MiB manifest; 2,048 cases; 64 KiB per case; 16 MiB per binary member; 64 MiB total
binary members; 32 levels of JSON/directory nesting; 16,384 JSON values per case; 32,768 manifest
values; 8 KiB per string; 192 ASCII bytes per relative path. The inventory is closed: every file below `vectors/`
must be referenced exactly once, apart from the manifest itself.

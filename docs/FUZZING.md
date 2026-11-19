# Fuzzing

Status: first pass, 2026-11-15. Coverage-guided fuzzing (libFuzzer, through `cargo-fuzz`) of the
byte-level decoders, to look for the inputs the hand-written tests did not think of. OA-GEN-004
says no parser panics on any input; this is the first search for a counter-example that does not
depend on us imagining it.

## The dependency exception

The project has zero external crates (RULESET F6, OA-GEN-002). Fuzzing needs a libFuzzer entry
point, and writing one ourselves is not sensible (it is the C++ LLVM runtime plus its glue). The
one exception, decided by the project owner:

- The only helper crate is `libfuzzer-sys`, and it lives only in `fuzz/`.
- `fuzz/` is its own Cargo workspace (`[workspace]` in `fuzz/Cargo.toml`) and is listed in the root
  workspace's `exclude`. `cargo build`, `cargo test` and `cargo clippy` at the root never see it.
- No shipped crate depends on it. The fuzz crate depends on ours (by path), never the reverse.
- `fuzz/Cargo.lock` is committed, so the exact versions are pinned and reviewable.

Everything in `fuzz/Cargo.lock` that is not one of our crates (10 packages; `libfuzzer-sys` pulls
the rest in, `arbitrary` is a non-optional dependency of it):

| Package | Version | Why it is there |
|---|---|---|
| libfuzzer-sys | 0.4.13 | the harness, builds the bundled libFuzzer |
| arbitrary | 1.5.0 | required by libfuzzer-sys; our targets take raw bytes and do not use it |
| cc | 1.11.2 | libfuzzer-sys build script (compiles libFuzzer) |
| find-msvc-tools | 0.1.15 | build dependency of `cc` |
| shlex | 2.0.1 | build dependency of `cc` |
| jobserver | 0.1.35 | build dependency of `cc` |
| libc | 0.2.191 | `jobserver` |
| getrandom | 0.4.3 | `jobserver` |
| cfg-if | 1.0.5 | `getrandom` |
| r-efi | 6.0.0 | `getrandom` (UEFI targets only, not built on Linux) |

The fuzz binaries are test tools. They are never built into a release artifact.

## Layout

```
fuzz/Cargo.toml          own [workspace]; path dependencies on the oa-* crates
fuzz/Cargo.lock          committed
fuzz/fuzz_targets/*.rs   one target per decoder entry point
fuzz/seeds/<target>/     small seeds (about 300 KB in all), safe to commit
fuzz/make_corpora.py     builds the local corpora from the raw data (stays out of the repo)
```

Corpora and crash artifacts live outside the repository, because the real samples are not ours to
redistribute (the ASTERIX captures are GPL-2.0, adsb.lol is ODbL, the rest has its own terms) and
the repository is Apache-2.0:

```
$OPENATCA_DATA/fuzz/<target>/corpus/       grows as libFuzzer finds new coverage
$OPENATCA_DATA/fuzz/<target>/artifacts/    crashes, timeouts, OOMs, slow units
$OPENATCA_DATA/fuzz/<target>/fuzz-0-<secs>.log   the libFuzzer log of the last run
```

## Running

Needs `cargo-fuzz` and a nightly toolchain (the sanitizer flags are nightly only). The first build
compiles libFuzzer; later ones are incremental.

```
cd fuzz
export OPENATCA_DATA=/path/to/openatca-data
python3 -I make_corpora.py                       # once, to fill the corpora
T=asterix                                        # one of the targets below
mkdir -p $OPENATCA_DATA/fuzz/$T/artifacts
CARGO_BUILD_JOBS=1 cargo +nightly fuzz run $T $OPENATCA_DATA/fuzz/$T/corpus -- \
  -artifact_prefix=$OPENATCA_DATA/fuzz/$T/artifacts/ \
  -rss_limit_mb=2048 -max_total_time=900 -jobs=1 -workers=1
```

To replay one input (a crash from `artifacts/`) under the sanitizers:
`cargo +nightly fuzz run $T path/to/input`. To shrink it: `cargo +nightly fuzz tmin $T path/to/input`.
To reduce a corpus that has grown large:
`fuzz/target/x86_64-unknown-linux-gnu/release/$T -merge=1 new_dir corpus_dir`.

The fuzz profile keeps `overflow-checks = true` (as the workspace release profile does) and
cargo-fuzz adds debug assertions and AddressSanitizer, so an arithmetic overflow, a slice index out
of range or an assertion is a finding, as is a timeout, an out-of-memory (2 GiB RSS limit) or a
memory error.

## Targets

Every target takes raw bytes into the public decoder and must never panic. A refusal with a typed
error is the correct outcome for bad input and is not a finding. Where a decoder keeps state
(the XML parser tables, the row tables) it is built once per process and reused, as in production.

| Target | Entry points | Notes |
|---|---|---|
| `asterix` | `oa_asterix::decode_block`, `decode_stream`, then `encode_block` of every decoded block | categories 21, 48, 62 and the generic block walker |
| `adsb-payload` | `oa_adsb::decode_payload_lenient`, `decode_payload`, then `reports_row` | the OAF1 history payload |
| `adsb-trace` | `oa_adsb::decode_trace` (readsb / adsb.lol trace JSON), with the ZAU box and with a keep-everything filter | this is the bytes-to-row entry for readsb rows; there is no separate row decoder |
| `feed-frame` | `oa_feed::FrameReader` push/next/finish | whole input and 7-byte chunks, so the resynchronising path and partial frames are both exercised |
| `grib2` | `scan_messages`, `decode_stream`, `inspect_stream`, `decode_message`, `sample_bilinear`, `parse_idx` | decode_stream runs template 5.40 through `oa-j2k` as well |
| `j2k` | `oa_j2k::decode` | JPEG 2000 codestreams |
| `tiff` | `oa_tiff::read_geotiff`, then `grid_elevation_at` | classic TIFF / GeoTIFF; there is no separate `open` |
| `xml` | `oa_xml::Parser::parse`, then namespace resolution of every element | default limits |
| `tbfm` | `oa_tbfm::decode_bytes` and `decode_doc_prefix`, then `row_at` | |
| `tfms` | `oa_tfms::decode_bytes` and `decode_doc_prefix`, then `flight_at` | |
| `fixm` | `oa_fixm::Decoder::decode` and `decode_prefix` | SFDPS messages |
| `tar` | `oa_tar::TarReader` `next_entry` / `read_entry_to_vec` | |
| `zip` | `oa_zip::Archive::parse_with_limits` (default and tight limits), `extract_entry`, `is_safe_path` | |
| `gzip` | `oa_zip::GzipReader`, `oa_zip::Inflater`, `oa_zip::inflate` | gzip members and raw DEFLATE, 64 MiB output cap |
| `awc` | first byte selects METAR, TAF, PIREP, SIGMET, AIRSIGMET or G-AIRMET; the rest is the file; parse then finalize | |
| `json` | `oa_json::parse`, then every node accessor | the parser under `oa-awc` and `oa-adsb` |
| `csv` | `oa_csv::Reader` (header, every record, every accessor), once with a `BufReader` capacity of 1-64 bytes taken from the first input byte and once with the default capacity | asserts the two give the same header, records and error, so BOM and record handling cannot depend on buffering |
| `arinc424` | `oa_arinc424::parse_bytes` and `parse_reader` (7-byte buffer), the file accessors on the result, `parse_record` and `record_meta` on each line (as is and padded to 132 bytes), `parse_lat`, `parse_lon`, `parse_path_terminator`, `parse_cycle` | |
| `nasr` | first byte selects a parser: airports, runways, runway ends, fixes, fix navaids, navaids, airway bases and segments, airways, ARB base and boundaries, class airspace, STAR/DP bases, airports and procedures, `parse_schemas`, `validate`; the rest is the CSV (form feed between files for the multi-file parsers) | every parser takes a `BufRead`, so the byte entry is the public one |
| `notam` | `oa_notam::decode_bytes`, then every row accessor, plus the `scalar` field parsers on the raw bytes | `cross_check` needs an `oa-mva` model and is not fuzzed |
| `nam` | `oa_nam::frame::read_header`, `unframe`, `parse`, `parse_tolerant`, then `encode` of every accepted message (byte-exact for `parse`, re-parse for `parse_tolerant`) | NAM ACC-to-ACC messages; seeds are the ICD examples |
| `mva` | `oa_mva::load_message`, then the model accessors and every query (`sectors_containing`, `min_altitude_at`, `min_altitude_with_sub`, `sector_distance_nm`, `sectors_within_radius`) at a vertex of the chart | |

Not fuzzed: `oa-dof`, `oa-airspace`, `oa-replay`, `oa-wind`. (`oa-notam`, `oa-mva`, `oa-nasr`,
`oa-arinc424` and `oa-csv` were added in a second pass, below.)

## Seed corpora

| Target | Seeds (local corpus only unless noted) |
|---|---|
| nam | 35 of the Rev G example messages, bare and framed, and one with the printed spacing (committed, `fuzz/seeds/nam`) |
| asterix | committed (`fuzz/seeds/asterix`, 46 files): 23 valid blocks and streams and 23 refused ones. The valid ones are the hand-built CAT021, CAT048 and CAT062 records of the `oa-asterix` tests (byte derivations in `tests/handbuilt.rs`), the untyped-item and typed-item records of `tests/common`, FSPEC variants, reserved-expansion and special-purpose items, and a three-category stream; the refused ones are the refusal cases of `tests/refusal.rs` and `tests/findings.rs` (block length, FSPEC, unknown slots, compound items, truncation). Locally, the GPL-2.0 captures in `raw/asterix-samples`, split into packets and blocks, never committed. |
| adsb-payload | three synthetic payloads (committed) |
| adsb-trace | committed (`fuzz/seeds/adsb-trace`, 32 files): 17 accepted traces, 3 accepted with refused rows (`rowbad-`) and 12 refused whole (`bad-`), all built from the `oa-adsb` decode tests: every row flag, altitude form and source type, old 7 to 9 column rows, the aircraft object at its limits and with malformed minor fields, box edges, non-ICAO and reserved addresses, and the structural errors. No adsb.lol data is committed (ODbL). Locally, 232 real adsb.lol trace files (ODbL), from the first part of the 2026-11-13 archive. |
| feed-frame | 200 real frames and 20 five-frame runs from the SWIM recordings, plus one synthetic stream (committed) |
| grib2 | the ecCodes and NCEP files in `raw/grib2-testdata`, whole and split into messages |
| j2k | committed (`fuzz/seeds/j2k`, 55 files): 20 codestreams that decode (the two frozen streams of the `oa-j2k` tests, and others made with the test encoder: sizes from 1x1 to 64x64, 1 to 16 bit, signed, image origin, layers, small and wide code blocks, constant and extreme images, an informative marker, Psot of zero) and 35 refused ones (unsupported markers, COD, SIZ, SOT and quantization fields set outside the profile, truncation, trailing bytes, damaged packets). Locally, the JPEG 2000 section-7 bodies cut out of the template 5.40 messages above. |
| tiff | four synthetic GeoTIFFs (committed): little and big endian, raw and deflate |
| xml | SWIM messages of each kind (TBFM, TFMS, SFDPS, FNS) and five synthetic documents (committed) |
| tbfm | committed (`fuzz/seeds/tbfm`, 35 files): 21 messages that decode (each section kind, all five in one flight, every message class, `air` change types, comment and CDATA split text, unknown elements) and 14 refused (namespace, root, XML, field and consistency errors, truncation); built from the made-up fixtures of the `oa-tbfm` tests |
| tfms | committed (`fuzz/seeds/tfms`, 32 files): 21 documents that decode (every flight message type and every flow message type, mixed documents, status and empty documents) and 11 refused (root, namespace, XML, field errors, DOCTYPE, truncation); built from the made-up fixtures of the `oa-tfms` tests |
| fixm | committed (`fuzz/seeds/fixm`, 29 files): 15 SFDPS documents that decode (message sources TH, HZ, FH, AH, OH, DH, CL, BA and LH, several messages, an empty collection) and 14 refused (whole-message rejections, field errors, other roots, non-XML, truncation); built from the made-up fixtures of the `oa-fixm` tests |
| tbfm, tfms, fixm (local) | 600 real messages each from the 2026-11-14 SWIM dumps, never committed |
| tar, zip, gzip | synthetic archives from Python's standard library (committed) |
| awc | committed (`fuzz/seeds/awc`, 29 files, first byte is the product selector 0 to 5): 13 files from the made-up fixtures of the `oa-awc` tests for all six products, 6 excerpts of two or three records from the public-domain AWC files of 2026-11-14 (`*-real-*`), and 10 refused (broken records, truncation, wrong top level). Locally, the AWC files in `raw/awc`, each prefixed with its selector byte. |
| json | committed (`fuzz/seeds/json`, 78 files): 32 documents that parse and 46 refused, written from the RFC 8259 grammar and examples (escapes, surrogate pairs, number forms, depth 64 and 65, duplicate keys, a trace-shaped object) and not copied from any suite. Locally, the JSONTestSuite files, adsb.lol traces and AWC files. |
| csv | eight synthetic files (committed; the first byte is the buffer capacity); locally, four chunks (header plus up to eight rows) of each NASR CSV file with a random capacity |
| arinc424 | six synthetic files (committed): the crate's fixture record layouts with the airport identifiers and names replaced, in files with a matching header count; locally, 400 chunks of 1-30 consecutive records of the real CIFP under its real header with the count patched |
| nasr | seven synthetic files (committed): a structure file, a structure plus data file, fixes (valid and hostile), STAR base, and the STAR and DP families as three files; locally, chunks of every parsed NASR file (header plus 1-40 rows), the STAR and DP triples, the airway pairs, every structure file and structure-plus-data pairs |
| notam | six synthetic messages (committed), built from the made-up fixture of the `oa-notam` tests; there is no local real NOTAM corpus |
| mva | four synthetic charts (committed): two squares, one with a hole; a FUS5 chart; a sub-area chart; an unclosed ring; locally, the FAA chart files in `raw/mva` and `raw/mva-zjx` |

The committed seeds of `asterix`, `adsb-trace`, `awc`, `fixm`, `j2k`, `json`, `tbfm` and `tfms`
(added 2026-11-18) are named `ok-*` (must be accepted), `bad-*` (must be refused with an error) and,
for `adsb-trace`, `rowbad-*` (the trace is accepted, at least one row is refused and counted); each
is under 64 KiB. They come from the tests' own fixtures and from the decoders' grammars; none is a
recording, apart from the six short AWC excerpts, and no real flight, aircraft or adsb.lol data was
copied (no LADD list is available to filter it against). Each crate checks its folder with
`tests/fuzz_seeds.rs`, which runs the same entry points as the fuzz target and asserts the prefix
of every file, so a decoder change that flips a seed fails `cargo test` instead of silently
shrinking the corpus. That test is not a fuzz run; it needs no nightly toolchain.

Each seed is under 400 KB and each corpus was under 50 MB at the start; after a run the `awc` and
`adsb-trace` corpora had grown past it and were merged back down (`-merge=1`).

## Results

First run, 2026-11-15: every target for 15 minutes (`-max_total_time=900`), one after the other, one
worker, 2 GiB RSS limit. Coverage is libFuzzer's count of covered edges (`cov`) and of distinct
features (`ft`) at the end of the run. The corpus column is the number of inputs libFuzzer kept at
the end of the run (not the size of the starting seed set).

| Target | Executions | exec/s | cov | ft | Kept inputs | Crashes, OOM, timeouts |
|---|---|---|---|---|---|---|
| asterix | 6,296,720 | 6,988 | 2,155 | 9,266 | 961 | none |
| adsb-payload | 13,352,743 | 14,819 | 525 | 1,512 | 54 | none |
| adsb-trace | 163,851 | 181 | 1,105 | 4,697 | 593 | none |
| feed-frame | 209,342 | 232 | 111 | 545 | 114 | none |
| grib2 | 4,040 | 4 | 1,771 | 7,202 | 212 | none (one slow unit, below) |
| j2k | 16,344 | 18 | 1,014 | 2,757 | 306 | none |
| tiff | 4,981,334 | 5,528 | 1,050 | 2,731 | 572 | none |
| xml | 6,475,522 | 7,187 | 1,038 | 4,388 | 1,478 | none |
| tbfm | 13,245,488 | 14,700 | 2,235 | 5,705 | 1,741 | none |
| tfms | 2,537,879 | 2,816 | 2,601 | 8,087 | 1,345 | none |
| fixm | 974,046 | 1,081 | 2,004 | 7,115 | 1,356 | none |
| tar | 19,385,062 | 21,515 | 521 | 1,122 | 266 | none |
| zip | 10,525,309 | 11,681 | 637 | 2,074 | 514 | none |
| gzip | 1,052,480 | 1,168 | 672 | 2,819 | 466 | none |
| awc | 306,709 | 340 | 4,022 | 14,896 | 1,424 | none |
| json | 5,861,705 | 6,505 | 557 | 2,669 | 766 | none |

No target found a panic, an arithmetic overflow, an out-of-bounds access, an out-of-memory or a
timeout. The only artifact is a `slow-unit` of the `grib2` target: a 1 MiB prefix of the public
NCEP GEFS file (27 messages), which takes about 6 s under the sanitizer and coverage build. It is
real, bounded work, not a defect: the whole GEFS file in the corpus takes about 10 s in the same
build for the same target, which runs the stream, inspect and single-message decoders over it,
and the work is capped by `MAX_MESSAGE_POINTS` and the J2K work budget. It is the reason the grib2
and j2k targets execute so few inputs per second.

Because nothing crashed, no regression tests were added and no code changed (the requirement
numbers for such tests would have started at each crate's highest plus 60).

### Second pass: text parsers, 2026-11-15

Smoke run of each new target for 60 seconds (`-max_total_time=60`, 2 GiB RSS limit), starting
from the committed seeds and the local corpus. A smoke run, not a campaign: it says the targets
build and run, not that the parsers are clean.

| Target | Executions | exec/s | cov | ft | Kept inputs | Crashes, OOM, timeouts |
|---|---|---|---|---|---|---|
| csv | 141,502 | 2,319 | 451 | 1,645 | 231 | none |
| arinc424 | 45,838 | 751 | 3,138 | 14,883 | 1,349 | none |
| nasr | 90,326 | 1,480 | 3,838 | 13,763 | 1,084 | none |
| notam | 1,038,538 | 17,025 | 2,376 | 6,619 | 1,368 | none |
| mva | 39,959 | 655 | 1,848 | 7,561 | 488 | none |


### Extended text and chart parser run, 2026-11-16

A sequential rotation requested 45 minutes per target, one worker and a 2 GiB RSS limit. Five
of six targets completed. The wrapper itself returned success despite an individual target failure,
so the table follows each target's log rather than the wrapper's exit code.

| Target | Executions | cov | ft | Kept inputs | Result |
|---|---:|---:|---:|---:|---|
| csv | 1,342,465 | 1,276 | 5,451 | 923 | completed |
| arinc424 | 481,418* | 3,353* | 17,018* | 2,053* | stopped early; harness panic |
| nasr | 3,186,394 | 4,279 | 17,231 | 2,148 | completed |
| notam | 29,354,525 | 2,843 | 9,326 | 2,691 | completed |
| mva | 3,036,676 | 2,517 | 10,189 | 1,549 | completed |
| tiff | 34,492,297 | 1,089 | 2,949 | 491 | completed |

`*` Last progress counters before the failure, not final campaign totals. The logs do not record
the source commit, so these results cannot establish coverage of an exact revision.

The ARINC 424 target panicked while preparing a fixed-width record: `String::truncate(132)`
split a two-byte UTF-8 character. The parser itself did not panic. Preparation now resizes bytes
and invokes the text-record decoder only when the prefix is valid UTF-8; the whole-file byte
parser still receives the original input. The exact input is committed as
`fuzz/seeds/arinc424/utf8-record-cutoff.bin`. It reproduces the old harness failure and completes
with the corrected target. A subsequent 61-second smoke run completed 147,470 executions with
no failure; it is not a replacement for the interrupted 45-minute run. AddressSanitizer remained
enabled, but leak detection was disabled because LeakSanitizer cannot operate in the test sandbox.

The TIFF artifact retained from an earlier rotation is the overflow input covered by
OA-TIFF-095. This rotation's TIFF log completed normally with no new crash.

### Completed ARINC 424 continuation, 2026-11-16

The corrected ARINC 424 target was run again for the full requested 45 minutes at
`dfa3c129b1be00c8bc5ab9905201059da76e497c`, starting from the grown local corpus and the
committed UTF-8 cutoff seed. One worker, `-max_total_time=2700`, and a 2 GiB RSS limit produced:

| Executions | Elapsed seconds | cov | ft | Kept inputs | Result |
|---:|---:|---:|---:|---:|---|
| 6,251,668 | 2,701 | 3,626 | 20,363 | 2,892 | exit 0; no failure artifacts |

The run ended at 19:22:54 UTC. AddressSanitizer was enabled; leak detection remained disabled
because LeakSanitizer cannot operate in the sandbox. The local campaign directory retains the
source revision, binary SHA-256, limits, log, corpus and exit status. This replaces the interrupted
ARINC campaign; it does not establish absence of parser defects or coverage of later revisions.

## What fuzzing does not show

- Absence of bugs. Fifteen minutes per target, one core, one run is a sample. In every target
  but `feed-frame` new coverage was still being found in the last ten percent of the run (the last
  new input came after 12% of the executions for `feed-frame`, at least 90% for the others), and
  `grib2` and `j2k` execute only a few inputs per second, so a longer run, or a second run from the
  grown corpus, will reach further.
- Correctness. The targets check that decoding does not panic, overflow or run away. They do not
  check that a successful decode is right; that is what the tests against the public reference
  files, the round trips and the formal proofs are for. The `asterix` target does feed every
  decoded block back through `encode_block`, but only to look for panics.
- Anything the targets do not call: only the entry points listed above, with default or stated
  limits. The limits themselves (a 1 MiB XML document, a 32 MiB JSON file) bound the work, so a
  failure that needs a larger input than the fuzzer's `-max_len` (4 KiB, raised to the largest
  seed) is not reachable here.
- Resource behaviour of the full system, I/O errors, or timing. Each target is a pure function of
  one buffer.
- State across calls beyond what each target reuses (parser tables, row tables). A sequence of
  calls that corrupts shared state is not searched for.
- Real-time behaviour: the 2 GiB RSS limit catches runaway memory, and libFuzzer's 20-minute
  per-input timeout catches a hang, but a decode that takes seconds instead of milliseconds is
  not flagged. The slow-unit note above is the one place that was looked at by hand.

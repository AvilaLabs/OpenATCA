# oa-stdds-ref

Scores the OpenATCA tracker and ADS-B truth against STARS terminal tracks published through
STDDS (TAIS). It reads an ADS-B clip and one or more STDDS recording files:

```text
oa-stdds-ref --adsb <clip.oaf> --out <dir> --cohort-id <id> --from-ms <inclusive-unix-ms> --to-ms <exclusive-unix-ms> <stdds.oaf[.gz]>...
```

It writes `stdds_ref.txt`, per-record `stdds_ref.csv`, and aggregate `stdds_ref.json`.

## Classes and meaning

STARS in fusion mode can use ADS-B as one of its sensors. Every record is reported in a separate
flag-derived class; the class name gives the recorded flag value, not an interpretation:

| Class | Rule |
|---|---|
| pseudo, frozen or coasting | any of `track/pseudo`, `track/frozen` or `track/coast` is present and true; reported apart |
| adsb=false | `track/adsb` present and false |
| adsb=true | `track/adsb` present and true |
| adsb absent | `track/adsb` absent |

An absent flag is never treated as false. `adsb=false` does not establish radar-only or independent
positioning; the flag's sensor semantics remain unconfirmed. No all-class score is produced.

## Comparison and cohorts

Each mode compares three error sets on one common cohort: tracker minus STDDS, ADS-B truth minus
STDDS, and tracker minus ADS-B truth. The raw mode uses each `mrtTime` as published. The fitted
mode reads the references at that timestamp minus a per-key offset fitted against ADS-B truth
only. The offset search is ±15 seconds in 0.1 second steps, ties toward zero, with at least eight
usable samples required. Fit results at a search boundary are marked.

Fitted offsets use the same records later reported in fitted mode. Fitted metrics therefore show
in-sample calibrated agreement, not prospective accuracy. TAIS has no generation identifier: a
silence longer than 60 seconds for the same positive address, facility and track number begins a
new fit segment. This is an explicit reuse heuristic, not proof of flight identity or generation.
Class changes split fits and are counted. All rows at a duplicate key timestamp are excluded as
ambiguous; descending file/record timestamps are counted. The offline sorted analysis does not
emulate live arrival timing.

Calibration requires all key parts: a positive address, facility identity, and track number. Rows
missing facility identity or track number can still pair by address for raw scoring, but are
explicitly excluded from fitting. Rows with incomplete keys are never pooled into a synthetic
track. A duplicate timestamp is excluded before both churn and arrival-order diagnostics, so file
ordering cannot choose which class appears to come next.

Pairing requires a positive Mode S address and never falls back to proximity, facility, or track
number. Missing and zero addresses are counted separately and excluded. Times use `mrtTime`, Unix
milliseconds UTC. Tracker rows interpolate linearly over gaps up to 5 seconds and extrapolate up
to 1 second at a track end; extrapolation is flagged and counted. ADS-B truth is never extrapolated.
Along and cross track use the truth's velocity direction (cross positive to the right); below
2 m/s those components are absent. Position errors are in metres, altitude differences in feet,
and velocity differences in m/s. STDDS altitude and velocity units are assumptions from
`crates/oa-stdds/src/tais.rs`; per-class checks compare altitude and speed distributions. Tracker
altitude may be barometric or geometric; the input row does not identify which.

## Outputs and provenance

The CSV contains file and record indexes, class, statuses, explicit `raw_common_cohort` and
`fitted_common_cohort` markers, the fit segment, and errors in named units. Sample timestamp,
address, track number, callsign and position are omitted. JSON contains per-class cohort and
exclusion counts by reason, timestamp basis, units, calibration policy and provenance lines. The
text summary reports aggregate statistics and history diagnostics. Raw and fitted cohorts can
differ; all three error sets share each mode's cohort. The report also provides raw and fitted
metrics over their same-record intersection, with a separate numeric denominator for every
metric because fields such as speed may be absent. Fitted results are in-sample calibrated
agreement, not prospective accuracy.

The command publishes `stdds_ref.txt`, `stdds_ref.csv`, `stdds_ref.json`, then
`stdds_ref.complete`. The last file contains SHA-256 digests of each exact report and the
provenance header. A reader should treat the three outputs as complete only when this marker
exists and matches. Files are staged and synced before publication; final names use atomic
no-replace hard links on the same filesystem. One same-host OS file lock admits one publisher at
a time. This lock is not distributed fencing. A crash can leave staged files or a prefix of final
outputs without the completion marker; the next invocation refuses existing outputs and requires
manual inspection/cleanup. A crash between a hard link and removal of its staging name can leave a
two-link file, which the sealed-data guard rejects until the stale stage is removed. Directory or
file races by an external writer are not prevented by this local lock.

Input bounds are checked before retained STDDS rows are scored: at most 32 files, 128 MiB stored
and decoded bytes per STDDS file, 384 MiB total stored STDDS bytes, 512 MiB decoded STDDS bytes,
and 100,000 retained STDDS rows. The ADS-B clip is limited to 128 MiB and 100,000 decoded report
rows. Report construction is estimated before analysis and capped at 256 MiB. A separate
80,000,000-unit work envelope accounts per fit segment for `min(segment rows, 128)` samples times
301 offset candidates, plus bounded report scans. It is checked before ADS-B tracker construction
and preserves the 100,000-record input ceiling for the conservative worst case.
Exceeding a limit is a visible refusal, not silent truncation. These limits are a bounded research
envelope, not a production-scale feed limit.

Input SHA-256 values, the running binary SHA-256, selected source/configuration hashes, settings,
and output digests are recorded. The source fingerprint explicitly covers the comparator source
files and workspace manifests/lock/toolchain file listed in the report; it is a partial
fingerprint, not a dependency closure or reproducible-build claim. The running executable must be
openable as a guarded single-link file; copy an installed binary to a standalone path when a build
system hard-links it into multiple locations. Every input and output path is checked by the shared
sealed-data guard before access.

## Pre-run plan

`plan_run.py` turns a metadata-only inventory into a private, hashed run plan before inputs are
opened. The JSON inventory records a cohort ID, explicit UTC interval, source commit/tree hash,
the executable path and digest, and each input path, digest, and saved provenance record ID. The
helper does not stat, enumerate, open, or hash any FAA input path. Its output freezes the current
raw and fitted modes, fit policy, tool limits, output filenames, and exact current command line.
Keep both inventory and plan private.

The interval is enforced as a half-open UTC `mrtTime` selection on STDDS rows before calibration.
The cohort ID and both signed Unix-millisecond bounds are required; unbounded scoring is refused. The `from` bound
is included and `to` excluded. An interval intersecting the compiled sealed time window is refused
before any recording is opened. Rows parsed from the STDDS inputs are accounted as selected,
outside-window, or untimed, in addition to the loader's sealed-window drop counter. The ADS-B clip
is loaded without that interval cut so it retains truth around the cohort boundaries. The fitted
mode remains an in-sample agreement result. Example and tests use synthetic paths and digests only.

## Recording status and limits

The STDDS subscription was approved 19 November 2026. A real 20 November preliminary census has
4.5 million updates across 142 facilities (`UK-009`). It is preliminary descriptive evidence, not
a completed STARS comparator result; it does not prove radar independence or validate a class
interpretation. This tool defines a comparison and emits machine-readable per-class output. It
scores TAIS terminal tracks, not ASDE-X surface tracks.

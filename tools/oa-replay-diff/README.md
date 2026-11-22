# oa-replay-diff

Compares two tracker runs of the same clip.

## Producing the files

```
oa-track-run --adsb clip.oaf --out dir --no-score --digest digest.txt --rows rows.bin
```

`digest.txt`: a `#` header line (format v2), then one line per output second (Unix second of the
row time): `second chain_hash second_hash rows`, then `end seconds rows last_chain`. `rows.bin`: a 24-byte header (`OATKROWS`, version `1`, row size
85, row count, little-endian) and the canonical 85-byte rows grouped by ascending second
(`oa_track::canon` lists the field offsets). Without the two flags `oa-track-run` writes nothing
extra.

## Comparing

```
oa-replay-diff digest-a.txt digest-b.txt [--rows-a rows-a.bin --rows-b rows-b.bin]
               [--expect expect.txt] [--summary summary.txt] [--max-detail 50]
```

Prints the report, then `[summary]` and `key=value` lines (also written to `--summary`).
Exit status: 0 identical (with `--expect`: every difference declared), 1 different, 2 error.

## Comparing replay witnesses

To compare two complete local `oa-replay --input-fingerprint` runs, copy exactly these three
lines from each run into a small text file, preserving their order:

```text
settings_fingerprint version=1 sha256=<64 hex digits> note=settings_and_ordered_feed_stream_kinds_only
input_fingerprint version=1 status=complete pipeline_frames=<count> pipeline_payload_bytes=<count> sha256=<64 hex digits> source_rejected=0 source_sealed_drops=0 semantic_sealed_drops=0 semantic_errors=0 bridge_reject_frames=0 note=diagnostic_only_not_completeness_or_cross_clock_equivalence
final state_digest=<64 hex digits>
```

Then run:

```text
oa-replay-diff --compare-witness-files witness-a.txt witness-b.txt
```

The mode reads at most 4 KiB from each regular file through the sealed-path guard. It accepts only
the three version-1 lines above, complete status, a positive admitted-frame count, valid counts and
digests, and zero refusal or sealed-drop counters. It prints `MATCH`, `SETTINGS MISMATCH`, `INPUT MISMATCH`, or `STATE MISMATCH`;
exit status is 0 for `MATCH`, 1 for a mismatch, and 2 for malformed or incomparable evidence.
Settings are compared first, then admitted frame count and ordered-input digest, then final state
digest. No witness contents or input payloads are printed.

This verdict compares only the supplied local evidence. Interpret it only when the two witnesses
come from complete runs with the same build, replay configuration, feed and stream grouping, and
the same fingerprint/state-digest versions. A match does not establish source completeness beyond
the reported counters, equivalence between independently clocked feeds, authority, or correctness
of data omitted before the pipeline.

Differences are classed by field:

| class | fields |
|---|---|
| position | `lat_e7`, `lon_e7`, `alt_ft` |
| velocity | `vel_east_mps`, `vel_north_mps`, `turn_rate_rps`, `vrate_fpm` |
| covariance | `pos_cov_*`, `vel_cov_*`, `alt_var_ft2` |
| flags | `flags` |
| model | `mu_h_*`, `mu_v_*` |

Rows are matched by track address, time and tick flag (and, among equals, by emission order); rows
left over with the same address and time but another tick flag are paired as a `flags` change.
Also reported: rows added, rows removed, seconds whose matched rows are emitted in another order
(`order`, whatever else changed), and ambiguous pairings (`ambiguous_pairs`: keys held by more
than one row, paired by emission order alone).

## What is trusted

Nothing without checking. A digest needs its `v2` header, its end line and every chain hash recomputed (a v1 digest is refused; there is no legacy path); sizes
are bounded (2^20 seconds, 2^20 rows in a second). A rows file is used only after every second's
hash and count are recomputed from its rows and equal the digest line, its header count equals the
digest's total and it has no bytes after its last row. Any failure is exit 2 naming the first bad
second. Output errors (a closed pipe) are exit 2.

## Declared differences

`--expect expect.txt` lists, one per line (`#` starts a comment), the classes a change may alter:
`position`, `velocity`, `covariance`, `flags`, `model`, `added`, `removed`, `order`, `ambiguous`. Any other
class that differs fails the comparison (exit 1). `--expect` needs both rows files.

## Two commits

```
tools/replay_compare.sh <commitA> <commitB> <clip.oaf> <outdir> [scratchdir] [--expect <file>]
```

Builds each commit in a detached worktree and runs `oa-track-run --no-score --digest --rows`.
`<outdir>` must be new or empty; a non-empty one is refused before anything is built, so no
file of an earlier comparison can be read as this one's. On a failure the two rows files are
removed.
Writes `report.txt`, `summary.txt`, `digest-A.txt`, `digest-B.txt`, the run logs and, where
`/usr/bin/time` exists, `time-A.txt` and `time-B.txt` (wall time and peak memory). The two
rows files are removed unless `REPLAY_COMPARE_KEEP_ROWS=1`. A commit from before the
`--digest`/`--rows` flags gets `tools/replay_compare_overlay.patch` applied in its worktree by
`tools/replay_overlay_apply.sh`, only if each patched file is the blob the patch was made against
(zero fuzz; any mismatch is a refusal). Builds use `--locked` with the `RUSTFLAGS` family cleared;
the report records that, the toolchain and the sha256 of the patch and of every patched file.

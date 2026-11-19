# oa-replay-diff

Compares two tracker runs of the same clip.

## Producing the files

```
oa-track-run --adsb clip.oaf --out dir --no-score --digest digest.txt --rows rows.bin
```

`digest.txt`: a `#` header line, then one line per output second (Unix second of the row time):
`second chain_hash second_hash rows`. `rows.bin`: a 24-byte header (`OATKROWS`, version `1`, row size
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

Nothing without checking. A digest needs its `v1` header and every chain hash recomputed; sizes
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
Writes `report.txt`, `summary.txt`, `digest-A.txt`, `digest-B.txt`, the run logs and, where
`/usr/bin/time` exists, `time-A.txt` and `time-B.txt` (wall time and peak memory). The two
rows files are removed unless `REPLAY_COMPARE_KEEP_ROWS=1`. A commit from before the
`--digest`/`--rows` flags gets `tools/replay_compare_overlay.patch` applied in its worktree by
`tools/replay_overlay_apply.sh`, only if each patched file is the blob the patch was made against
(zero fuzz; any mismatch is a refusal). Builds use `--locked` with the `RUSTFLAGS` family cleared;
the report records that, the toolchain and the sha256 of the patch and of every patched file.

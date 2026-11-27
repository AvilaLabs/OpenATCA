# Local OAF observer package

This is a prepared, user-scoped Debian systemd package. It does not connect to a facility machine,
create subscription credentials, or fetch a feed. It needs the existing `oa-feed` executable,
the existing SCDS bridge JAR, and the already approved per-feed subscription files.

## Feed fan-out

The bridge is one consumer per SCDS queue. The three `openatca-collector@FEED` services each run
one `oa-feed`/bridge pair for the existing `SFDPS`, `STDDS`, or `TBFM` subscription. Each collector
writes the consumed OAF2 frames once, uncompressed, into its own size-capped journal directory.
There is no second broker consumer for the observer or Nov 30 capture. Journal segments are
created with unique names and never rewritten or removed by this package; only the active segment
is appended. The observer cursor and the capture cursor keep independent offsets over the same
local segment journal and advance across ordinary rotation. A missing cursor segment is counted
and reported, never silently treated as a continuous history. The capture manifest
references byte ranges of complete original frames; it does not rewrite timestamps, re-encode
payloads, or widen the declared `[from,to)` receive-time interval. The pin names every referenced
segment. No cleanup job is installed, and the recorder never deletes journal files; hitting its
configured cap latches that feed as an incomplete run; the B5 mode exits immediately and blocks another connection until inspected.

The `oa-feed` bridge ACK happens after the OAF frame is written and flushed to the pipe, before the
local recording write is made durable. Closed raw segments request file and directory sync. This
is not an end-to-end durable broker acknowledgement or a hardware power-loss guarantee. B5 invokes
`--stop-on-recording-failure`: on cap, write, flush, or sync failure `oa-feed` immediately stops
processing, terminates the bridge, counts the cap-triggering/lost frames and accepted queued bytes,
and exits with the dedicated status 75. The runner writes a per-feed failure latch; preflight
refuses to restart that collector until an operator investigates, resolves capacity/storage and
explicitly clears the latch and manually starts that one service again. To do so, inspect its
service journal, resolve the storage/rate cause, then remove the exact per-feed marker and run
`systemctl --user start openatca-collector@FEED.service`. The frame that triggers a cap and bytes
already accepted by the pipe may already have been JMS-acknowledged before durable recording, so this mode limits further loss
and reports uncertainty; it cannot make the bridge's earlier pipe ACK durable. Never describe pipe
ACK as durable recording.

## Observer outputs and limits

The observer polls each directory every two seconds, with a 16 MiB and 10,000-frame work limit per
feed per poll, a 16,384-entry segment directory cap, a 4 MiB frame payload ceiling, and a 20,000
capture-span cap. It stores its resumable cursor and a separate capture cursor under the user data
directory. Each cursor is atomically replaced after a bounded poll. A process restart resumes at
the saved file offset; an unfinished final frame is revisited, while a partial tail in a segment
followed by a later segment is counted as truncated. A system shutdown can leave the active
recording segment without its close-time sync; the next segment/cursor pass accounts for a partial
tail instead of treating it as a complete file. Missing cursor targets, malformed bytes,
CRC failures, frame sequence resets/gaps, recorder receive-time regressions, and inter-frame
arrival gaps over 60 seconds are counted.

`reports/observer.json` distinguishes the observer's scheduled 30-day window, its process
invocation start, each collector's systemd active-since time, and each feed's last-frame silence.
Adapter STATUS frames count connected/disconnected/reconnect events. Silence alone is an
arrival gap, not proof of a broker outage. JMS timestamp deltas are labeled as receipt-minus-JMS
diagnostics only; the observer does not decode source payloads, source event times, or claim
message-level late availability. Its frame availability is the availability of OAF frames in the
local journal.

Each collector has a 2 GiB default per-feed recording cap and 256 MiB file rotation. Before the
bridge opens, `ExecStartPre` requires positive measured uncompressed journal growth rates in
`~/.config/openatca-observer/storage-budget.env` for all three feeds. It applies an explicit 200%
safety factor over 720 hours (30 days), refuses if the target feed's remaining cap cannot hold
that budget, and requires free disk for the sum of all three 30-day budgets plus a 512 MiB reserve.
The example file is copied as `storage-budget.env.example`; the operator must measure local rates,
copy it to `~/.config/openatca-observer/storage-budget.env`, then set the three integer bytes/hour
values. Zero or absent
measurements fail closed before subscription consumption. Thus the default cap is accepted only
when those measured inputs prove it fits. Journals are uncompressed and not automatically pruned.
If the process stops abruptly during an active file, the next segment and cursor report allow the
partial tail to be counted as truncation; only closed segments request sync. Treat a 75 exit/latch
as an incomplete capture rather than restarting past it. Systemd limits each collector to 1 GiB RAM and the
observer to 512 MiB. The sleep inhibitor blocks suspend while each service runs. The 30-day
observer deadline persists across service restarts; a restarted service resumes its original
window rather than starting another 30 days.

If the capture cursor reaches its bounded span limit, it writes `state/capture-failed.json` and
marks the manifest `FAILED_INCOMPLETE`. The observer keeps updating frame availability but disables
capture retries, so systemd does not loop on the same overflow and the manifest cannot look complete.
Preserve the failure record and partial manifest. To retry, first make a reviewed change that raises
the bounded span capacity, save the old failure/manifest/cursors, then explicitly remove only
`state/capture-failed.json` and restart `openatca-observer.service`. If the revised capacity is still
insufficient, the failure is latched again. Never edit `status` to claim completion.

The package unit pins the capture acquisition range to `[2026-11-30T05:00:00Z,
2026-12-01T06:00:00Z)`. The manifest separately records P010 warm-up `[05:00,06:00)` on Nov 30
as excluded from scoring and the scored interval `[2026-11-30T06:00:00Z,2026-12-01T00:00:00Z)`; the extra acquired
post-score interval is explicitly labeled. The independent capture cursor starts at the oldest
available segment so it can catch up from earlier local history. `archive/capture-manifest.json`
remains `IN_PROGRESS` and makes no completeness claim; a separate close/check procedure must
verify that all feeds reached the declared end. A missing or late source message cannot be inferred
from an OAF-only cursor.

## Install on the future spare

Prepare a local transfer bundle from explicitly supplied, already-approved artifact paths. The
script requires expected SHA-256 pins, reads only those two artifact files, and atomically publishes
a new bundle directory. It includes package scripts/systemd templates, the binary/JAR hash inventory,
the storage-budget template, and the fixed capture/scoring bounds. It never reads subscription files
or copies runtime configuration. It does not contact a destination or install/start services.

```sh
./prepare-transfer-bundle.py \
  --oa-feed /explicit/path/to/oa-feed \
  --oa-feed-sha256 <reviewed-oa-feed-sha256> \
  --bridge-jar /explicit/path/to/scds-bridge.jar \
  --bridge-jar-sha256 <reviewed-bridge-jar-sha256> \
  --output /explicit/new/path/openatca-observer-transfer
```

The output must not already exist. Verify `SHA256SUMS.json` after any later transfer. A destination
account, host, and transfer directory must be supplied separately before a human performs any
transfer. The included `DEPLOYMENT-PROCEDURE-TODO.md` lists later operator steps; it intentionally
does not automate them.

For a later local installation, copy the package to the Debian user's home, then run:

```sh
./install-user.sh
```

The script copies only package code and unit templates, creates private data directories, and runs
`systemctl --user daemon-reload`. It does not enable or start units. After providing the existing
binary, JAR, and external subscription files, review the preconfigured capture bounds and start
using the commands printed by the installer. The collector unit requires the explicit measured
rate file before it can start. For unattended service survival, the Debian administrator
must enable user lingering for that account.

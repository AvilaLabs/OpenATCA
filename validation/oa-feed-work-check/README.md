# oa-feed bounded-work fixture

This standalone validation binary measures deterministic in-memory `FrameReader` service turns.
It links only the local `oa-feed` path dependency and uses no unsafe code or external crates. It
does not open, list, or depend on recorded feed data.

## Declared measurement

Before it starts any timed service call, the driver prints a **1,000,000,000 ns experimental
service-turn limit**. This is a fixture threshold, not a published `oa-feed` latency SLA and not a
universal hard-real-time claim. Each turn times the shared allowance across input push and frame
drain. Fixture construction and output formatting happen outside timed turns. The finalizer is
timed as a separate small operation.

Each case is limited to eight service turns. The driver stops on a nonrenewable refusal, unexpected
refusal count, counter mismatch, failed ownership equation, no-progress condition, or a turn reaching or
exceeding the declared limit. A passing run contains at most 32 measured service turns and four separately
timed finalizers, each strictly below one second. Fixture construction and formatting are outside
these timers. These checks run after each operation returns; the coordinator also supplies an
outer process timeout for execution.

## Cases

1. A maximum OAF2 frame with a 4 MiB payload and valid CRC.
2. **Malformed-input safety:** a maximum OAF2 frame with a damaged CRC, followed by maximum buffered
   resynchronisation. The frame is counted as a CRC error, and its rejected bytes are assigned to
   the skipped-byte total plus the three bytes retained before finalization.
3. **Malformed-input safety with valid-tail recovery:** two corrupt maximum OAF2 candidates whose
   starts overlap by 44 bytes, followed by a valid tail. A fresh finite allowance is applied only
   at service-turn boundaries; refusals retain the candidate, rejected prefix bytes are counted
   as skipped, and retry must deliver the valid tail exactly once.
4. A small valid frame, a maximum OAF2 frame, and a final small valid frame in one input sequence. The one-frame
   reader buffer forces compaction after consuming the first frame; shared copy/CRC limits cause
   service-turn deferrals and resumption while all three valid frames are delivered exactly once.

The driver checks offered = accepted + unoffered bytes and accepted = framed raw bytes + skipped
bytes + retained reader bytes before finalization. For malformed-input cases, CRC errors and
skipped bytes are explicit rejection dispositions; these are safety fixtures and do not count as
zero-loss valid-capacity measurements. For valid-only cases, every offered frame must be delivered
exactly once, in the expected sequence order, with raw bytes equal to the offered source range.
The timed region includes these oracle comparisons, so their overhead is included in the reported
service time. It then accounts for finalization, requires all source bytes to be accepted and all
reader bytes to be disposed, and checks exact frame/error, CRC-byte, scan-position, copy-byte,
compaction-byte, and refusal totals. A refusal is renewed only when the matching fresh-profile
allowance can satisfy it.

Work counters are unit counts: candidate positions, bytes sent to CRC, bytes copied, and bytes
moved during compaction. They are reported separately from `max_turn_ns` wall time; no work-unit
count is converted to a time estimate.

## Resource projection and scope

The fixtures offer about 16 MiB total input: two maximum-frame cases, one overlapping-frame case,
and one compaction case. They run sequentially. Peak live fixture storage is projected below 32
MiB: one source vector (at most about 4.2 MiB for the overlapping pair), one reader buffer (at most
about 8.4 MiB), and temporary maximum payload construction (4 MiB). No source-data reads occur.

The output includes the maximum observed wall time per service turn and the exact maximum observed
work counters per turn. These measurements cover in-memory framing only and characterize these
finite synthetic cases on the machine and build used. They do not establish performance for
arbitrary feeds, other platforms, or every possible `FrameReader` input. Gzip work, recorder writes
and flushes, rotation, and child shutdown are outside the timed path and have no deadline claim
here. Offline replay may renew between bounded steps while searching an arbitrary-length file;
this fixture does not claim a whole-file deadline for that search.

## Run

From the repository root, use the guarded single-job cargo command required by the lane rules, with
an external target directory, for example:

```sh
CARGO_BUILD_JOBS=1 /path/to/resource-guard cargo +1.89.0 run --release --locked --offline --manifest-path validation/oa-feed-work-check/Cargo.toml --target-dir /tmp/oa-feed-work-check-target
```

Keep the exact command, Rust version, source revision, fixture output, and binary hash together as
measurement evidence. This fixture must not be used to claim a universal deadline.

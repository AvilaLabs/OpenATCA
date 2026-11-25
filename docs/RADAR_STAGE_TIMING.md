# Radar stress shell-stage timing scope

Use `oa-perf one --stage-timings` for the queued synthetic radar run. The flag is opt-in, restricted to `one`, and leaves the default timing path and replay inputs unchanged. The per-second CSV adds `fusion_advance_wall_ms` and `shell_output_row_accounting_wall_ms`; the KV/text output gives measured-window distributions and their scopes.

## What these times mean

- `stage_timing.fusion_advance.*_ms` times only the shell call to `fusion_advance`, per measured traffic second. It excludes the preceding `fusion_adsb`/`fusion_plots` feed loop and includes every operation inside that core call: pending bucket processing, radar association, initiation, tracker advancement, and core `TrackOutput` generation.
- `stage_timing.shell_output_row_accounting.*_ms` times the shell loop that reads returned rows to compute age and continuity accounting. It does not measure core output generation, vector clearing, report formatting, or disk writes. It is outside the existing per-second capacity interval.
- `wall_ms` retains its existing scope: input offers plus the complete `fusion_advance` call. It includes the added `fusion_advance` timer's two monotonic clock reads when the option is enabled. The separate row-accounting timer adds two more reads outside that interval. No overhead subtraction is applied.

The internal requested phases—index build, index query, exact gating, assignment, initiation, tracker advance, and core output generation—are not separately timed. At this API boundary they all occur inside the pure `fusion_advance`/tracker call. Adding a clock to those core functions would violate RULESET B2–B4; callbacks or a wall-clock interface would violate B5. Splitting the phase API would alter a mature core boundary and replay/ordering semantics. The output labels say `not_separately_timed_at_current_pure_core_boundary`; the aggregate must not be presented as any one substage's time. Existing charged-work counters remain useful for bounded-work accounting, but they are not CPU or wall time.

The stage evidence vectors are opt-in, preallocated to the measured duration after a checked conversion, and capped at 86,400 measured seconds (four `f64` values per second across two summaries and the per-second pairs, under 3 MiB). Longer timer requests fail before traffic generation. This prevents timing evidence from growing without a declared bound.

## Reading the run

The stage distributions use one sample per completed measured second, not warm-up seconds. Per-second CSV rows align to the same measured-second rows as offered ADS-B/plots and emitted rows. A truncated or loss-bearing run remains a failed capacity qualification even though its observed timings are retained. GDB/MI wall-stack samples can help rank call paths inside the aggregate, with observer overhead disclosed; they do not turn the aggregate or charged units into per-stage CPU percentages.

This is shell instrumentation (RULESET B1/B3/C6), uses no dependency, and leaves the core pure (B2–B5). It adds four monotonic reads per measured second while enabled, with the first two inside the existing timed region (G1/G5). It preserves loss accounting and does not label a fast lossy run as a pass (G6). Synthetic source test coverage checks output availability and scope labels; no tests or measurements were run by the author.

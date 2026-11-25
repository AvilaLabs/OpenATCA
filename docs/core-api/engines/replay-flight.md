# Flight-state, feed-replay and offline clip API proposal

**Tier:** L3 Experimental / development tool; outside the proposed 0.1 format/library release. Names in the proposal section are not frozen, approved, or Stable. This page records actual source boundaries to prevent the separate components from being conflated.

## Flight state and SFDPS adapter

`oa-flight/src/lib.rs` currently re-exports `flights_new`, `flights_apply`, `flights_check`, `flights_digest`, `flight_get`, `flight_by_address`, `flights_remove`, `flights_sweep`, and plain types including `FlightKey`, `Flight`, `FlightConfig`, `Op`, `Stamp`, `Conflict`, `Refusal` and `Removal`. It models controller-relevant operations on one flight keyed by GUFI or the full TFMS fallback (callsign/departure/arrival), and retains one plan view per reporting center (current `MAX_VIEWS=8`). `oa-flight-sfdps` exports the `row_ops` adapter and offline SFDPS replay helpers; its function names are listed in that crate's `lib.rs`.

Flight operations carry signed Unix nanoseconds and source/message identity stamps; altitudes are feet and altitude forms remain distinct (not stated, cleared, VFR, simple, block, at-fix). The state is bounded at construction with three indexes and bounded logs; a full table refuses, it does not silently evict. Refusals/conflicts/removals are queryable records and counts. Source docs state no allocation after construction, no panic, and deterministic state for the same ordered operations; those source assertions are not the completed §8 evidence bundle. The adapter's interpretation of FAA SFDPS is a profile, not an encoder or general FIXM implementation.

## Feed replay (`oa-replay`)

`oa-replay` is currently a split crate with core tables/clock/errors/feed types and shell adapters, pipeline, source, report and CLI. Public source exports include `new_pipeline`, `process_frame`, `process_frame_mono`, `pipeline_drain_checkpoints`, `replay_hash`, `replay_state_digest`, stream merge/source helpers, and bounded feed tables. It reads `.oaf` and `.oaf.gz` streams, orders frames by supplied receive Unix ns, decodes SFDPS/TBFM/TFMS/FNS/ADS-B, applies them to bounded tables, and counts errors by kind. Default checkpoint spacing is 10 s; minimum 1 s; default ring 8,640 checkpoints; a document is capped at 4 MiB payload, 262,144 XML elements, 524,288 attributes, 4,096 rows, and 1 MiB route string bytes. A full checkpoint ring drops oldest undrained checkpoints and counts drops. `PipelineError`, `ErrKind`, table configuration errors, stream I/O/truncation and decoder refusal must stay distinguishable.

This is an internal recording format and replay utility, not a general network feed server, live transport, or public standard interface. It currently depends on `oa-sealed` and custom OAF framing. It does not provide a universal standards-independent feed abstraction. Replay time and receive time must not be collapsed; message, position and receive timestamps have different semantics.

## Existing `openatca-core` replay wrapper

The existing `crates/openatca-core` package exposes `SurveillanceReplay::open(path, ReplaySettings)`, `next_frame()`, `ReplayFrame::tracks()` and input/tracker health. It is experimental: offline only, ADS-B OAF clip only, reads the entire clip before playback, default caps 64 MiB and 500,000 declared report rows, and uses interval statistics to configure the tracker. It is not the umbrella Core library definition, not radar fusion, not flight-state processing, and not a multi-feed replay API. The README calls it a research/simulation/offline-analysis wrapper; preserve that limitation.

## Provisional public shape

For future review, separate `flight-state` (plain operations and bounded state), `feed-replay` (caller-supplied normalized frames and an explicit clock policy), and the ADS-B clip example/tool. A candidate `FlightStore::apply(Op)` surface or `ReplayPipeline::process(Frame)` is only a design sketch; these names are not implemented promises. A public replay module should not require an OpenATCA-only frame format; standard feed formats or documented plain values should cross its boundary. Keep path access, format decoding and time ordering in explicit shell components.

## Tier and release gaps

Neither the engine modules nor the replay wrapper are in §10's proposed 0.1 L0/L1 format scope. Before a later tier promotion: remove `oa-sealed` dependencies from public boundaries; enumerate FIXM/TBFM/TFMS/FNS/ADS-B versions; document all ordering, lag, duplicate, staleness, capacity, checkpoint and loss semantics; finish §8 references, fuzz, applicable Kani, mutation, no-panic, x86-64/aarch64, examples and language-neutral vectors. A deterministic replay digest is not an authenticity or provenance signature, and replay-call timing is not live system latency.

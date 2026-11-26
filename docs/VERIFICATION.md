# Formal verification

This is the first set of machine-checked proofs in OpenATCA. It is narrow on purpose: each proof
states a property, the bound it holds up to, and what it does not say. Nothing here is a
certification claim.

The registered tracker/output/timestamp assurance suite is described in
[Rust assurance](RUST_ASSURANCE.md). Its source-bound runner keeps exact harness names, required
branch covers, stubs, capacity configuration and toolchain identities with each result. This suite
extends the earlier inventory below; it is not a claim that all Rust code is proved.

## What Kani is

[Kani](https://github.com/model-checking/kani) is a bounded model checker for Rust. It compiles a
harness (an ordinary Rust function marked `#[kani::proof]`) to the CBMC model checker's input.
Inputs the harness takes from `kani::any()` stand for every value of their type at once; CBMC
searches all of them, with every loop unrolled to a stated bound (`#[kani::unwind(n)]`), for a
panic, an arithmetic overflow, an out-of-bounds access, or a failed `assert!`. If none exists, the
property holds for all inputs within the bound. If one does, Kani prints the input.

Harnesses are compiled only under `#[cfg(kani)]`. They are not in any shipped build, and
`cargo build`, `cargo test` and `cargo clippy` do not see them. The harness code is in
`crates/oa-asterix/src/proofs.rs`, `crates/oa-asterix/src/proofs_wire.rs`,
`crates/oa-adsb/src/frames.rs`, `crates/oa-feed/src/core/health.rs` and
`crates/oa-replay/src/core/clock.rs`, `crates/oa-track/src/time_gate.rs`, `crates/oa-track/src/fixed_map.rs`,
`crates/oa-math/src/trig.rs`, `crates/oa-truth/src/shell/clip.rs` and `crates/oa-runway/src/occupy.rs` (each at the end, in a `proofs` module).

Each harness is tagged `// REQ: OA-...-1nn` directly above `#[kani::proof]`. `oa-trace` links the
harness to the requirement and lists it as a proof harness, apart from tests. It does not run
Kani; the results are the table below.

Python tests in `test_*.py` modules use `# REQ: OA-...` directly above `def test_*`. They link as tests, with malformed/non-test tags refused. Traceability records linkage; it does not execute Rust, Python or proof checks.

The registered `oa-track-core` suite also uses `types/proofs.rs`, `tracker/proofs.rs`, `tracker/proofs/rollback_paths.rs` and `time_gate/proofs_full.rs`, all reached only through their exact `cfg(kani)` module declarations. Its caller, late-fixture and full-capacity sensor claim boundaries are stated in [Rust assurance](RUST_ASSURANCE.md).

## Which method for which property

Kani is used where a bounded search of every input is both feasible and the strongest check
available:
- parsers and decoders of untrusted input (ASTERIX, the ADS-B frames, the feed frames), whose inputs
  are bounded by nature;
- integer and state logic: the clock, the time gate, fixed-capacity tables and rollback;
- the absence of panics.

Floating-point geometry and the safety predicates built on it (conflict alert, MSAW, runway occupancy)
are not given Kani proofs as a rule. Bounded proofs of symbolic doubles mostly do not finish, and the
ones that do have proved small properties. They are checked instead by:
1. seeded property sweeps over the whole input domain, with the edge cases kept as separate tests;
2. a search for missed or late alerts, each failure kept as a regression test (being built for
   conflict alert and MSAW);
3. dense tests against exact or high-precision oracles.

A property that was proved and is now tested says so in its requirement. On 2026-11-20 eleven proofs
were retired this way and their properties moved to sweeps of 20,000 to 40,000 cases. The ids were
`full-sensor-live-count`, `inhibition-in-force`, `inhibition-band`, `ca-horizontal-now-step`,
`ca-predict-ttl`, `ca-chord-d2` and five of the runway proofs.

## How to run

Kani 0.68.0 (CBMC 6.11.0) must be installed (`cargo install --locked kani-verifier`, then
`cargo kani setup`). One harness at a time:

```
cargo kani -p oa-asterix --harness fspec_cat048_5
cargo kani -p oa-replay  --harness clock_observe_total_and_confirms
cargo kani -p oa-feed    --harness observe_seq_accumulation_saturates
cargo kani -p oa-adsb    --harness decode_row_total_ads2
cargo kani -p oa-adsb    --harness payload_records_ads3_4 -Z stubbing
cargo kani -p oa-track   --harness gate_step_back_needs_span_count_and_spread
```

The two `payload_records_*` harnesses use `#[kani::stub]` and need `-Z stubbing`. Several
harnesses take minutes and up to 5 GB; run them one at a time and under a memory limit. Kani builds
into `target/`.

## Results

Run on one laptop, single job, CBMC 6.11.0. Time is wall clock including compilation; memory is the
peak resident size of the run. Every harness below verified successfully (no counterexample).

These are the runs as first recorded. Registration is a separate step: only a harness in
`tools/rust_proofs.json` is run by CI, and the registry records its own CI time per proof
(`ci_seconds`). Registered on 2026-11-20 from this table: the ASTERIX primitives, time and
identification round trips, block framing at three octets, the CAT048 walk and the three FSPEC proofs,
and the five ADS-B proofs. Not registered:
- `walk_every_item_cat021_24` and `walk_every_item_cat062_24`. Under the registry runner both ran out
  of their 900 s budget without a result, so they count as unproved until they finish there;
- the clock, time gate, `fixed_map`, trigonometry, clip and feed-sequence harnesses, which have not yet
  been run under the registry runner.

| Harness | Property | Bound | Result | Time | Peak memory |
|---|---|---|---|---|---|
| `primitives_rd_take_sext` (oa-asterix) | `rd`, `take`: no panic, overflow or out-of-bounds read for any start and length; position moves only on success, by exactly `n`. `sext` keeps the low bits and stays in range | input up to 6 octets, `n` up to 4 (`take`: any `n`), `sext` every value and width 1 to 32 | proved | 0:10 | 297 MB |
| `fspec_cat048_5` | FSPEC parse: non-empty FRN list with the position inside the input, or an error positioned inside the input; no panic or overflow | CAT048: input up to 5 octets, which is every input (the FSPEC ends or is refused within the 4 octets the UAP can address) | proved | 6:39 | 2.5 GB |
| `fspec_cat021_3` | same | CAT021: input up to 3 octets (the UAP can address 7) | proved | 6:14 | 5.1 GB |
| `fspec_cat062_3` | same | CAT062: input up to 3 octets (the UAP can address 5) | proved | 3:58 | 2.6 GB |
| `walk_every_item_cat021_24` | `walk_field` over every slot of the UAP (fixed, extended, repetitive, explicit, compound, SP, RE): no panic, overflow or out-of-bounds; a success ends after its start and inside the input; an error is positioned inside the input | input up to 24 octets, start at 0 | proved | 1:52 | 1.7 GB |
| `walk_every_item_cat048_24` | same | same | proved | 0:50 | 724 MB |
| `walk_every_item_cat062_24` | same | same | proved | 8:34 | 4.1 GB |
| `parse_block_any_bytes_up_to_3` | block framing (`parse_block`): short header, LEN below three, past the input, no record, unsupported category; an accepted block is longer than zero and no longer than the input | input up to 3 octets, any category | proved | 0:23 | 527 MB |
| `ident_roundtrip_any_six_octets` | six octets that decode as an identification encode back to the same six octets | every six octets | proved | 0:16 | 327 MB |
| `time_roundtrip_24_bits` | `ms_to_time128(time128_to_ms(t)) == t`; neither conversion panics | every 24-bit time (the width of the data items); no-panic for every `u32` | proved | 0:39 | 181 MB |
| `decode_row_total_ads2` (oa-adsb) | record decoder: no panic or overflow; a kept row has an in-range position, a known or `unknown` or absent source, anomaly bits among the three that exist | every 80-octet record | proved | 0:14 | 251 MB |
| `decode_row_total_ads1` | same for the first layout | every 64-octet record | proved | 0:07 | 213 MB |
| `parse_header_any_bytes_up_to_2_records` | payload header check: no panic; an accepted header means length is exactly header plus count records | any bytes up to 176 octets (header and 2 records) | proved | 0:15 | 328 MB |
| `payload_records_ads3_4` | `decode_payload_lenient` and `decode_payload`: no panic or overflow, kept + dropped = records, each anomaly count at most kept, strict reports only a record below the count | valid header, exactly 4 records, the current (`ADS3`) magic; the records are all zero and the stubbed `decode_row` chooses keep or drop, so the counts are checked, not the record content; `reports_push` stubbed (see below). The previous (`ADS2`) magic shares the header branch and is not proved separately | proved | 4:54 | 5.7 GB |
| `payload_records_ads1_4` | same | same, `ADS1` (64-octet records) | proved | 2:49 | 3.2 GB |
| `observe_seq_accumulation_saturates` (oa-feed) | `observe_seq`: no panic or overflow; a forward jump adds one gap, the saturated missing count and records that gap; a late frame inside the last gap takes exactly one off `missing`, counts as reordered and not as a regression, and leaves the expectation; the open gap stays inside `missing` and below the next expected number; `missing` never exceeds the sum of every gap length added (a ghost total) | every `seq`, every `frames`, `seq_next`, `seq_last`, `gaps`, `missing`, `reordered`; `duplicates`, `regressions` and `reordered` below `u64::MAX`; the gap record inside `missing` and below `seq_next` (none before the first frame); `missing` at most the ghost total | proved | 0:12 | not re-measured |
| `clock_observe_total_and_confirms` (oa-replay) | `clock_observe` over the per-feed clock: no panic or overflow for any feed index, `now` and `t`; a step carries the saturated difference, comes only from a candidate count of at least `CLOCK_CONFIRM_FRAMES - 1` in the frame's own feed, clears every count, and none comes from an empty candidate state; a frame never adds candidates to another feed; every count stays at most `CLOCK_CONFIRM_FRAMES` | any `usize` feed, every `i64` pair, five feeds each with `cand_n` at most `CLOCK_CONFIRM_FRAMES`, counters below `u64::MAX` | proved | 0:17 | not re-measured |
| `clock_advances_by_at_most_the_tolerance_unless_the_feed_is_alone` | from a state the pipeline keeps (every feed with an accepted frame has it at or before the clock), an accepted frame moves a running clock forward by at most `CLOCK_TOLERANCE_NS`, or by at most `FEED_GAP_MAX_NS` when no other feed has ever had an accepted frame (`kani::cover!`: a move over the tolerance by a lone feed is reachable), and the state stays consistent with the resulting clock | same, plus that state | proved | 0:39 | not re-measured |
| `clock_step_needs_three_candidates_and_the_vote` | a step comes only after a run of `CLOCK_CONFIRM_FRAMES` candidates; a forward step needs nothing more (three forward candidates in a row are always a step); a backward step also needs at least as many backers of the new time as of the old clock; a refused step is a held backward frame counted in `steps_outvoted`, with the run kept at the limit and the vote against it (`kani::cover!`: forward step, backward step and refusal are reachable) | any `usize` below the feed count, every `i64` pair, `i64::MIN` excluded as the clock, five feeds in any state with `cand_n` at most `CLOCK_CONFIRM_FRAMES` | proved | 0:53 | not re-measured |
| `clock_observe_three_candidates_before_a_step` | from an empty candidate state of one feed, no step on the first two frames; a step on the third only after two held frames on one side of the clock, each within `CLOCK_TOLERANCE_NS` of the one before when behind it, or not more than the tolerance before and not more than `FEED_GAP_MAX_NS` after the one before when ahead of it | every three frame times, clock fixed, `i64::MIN` excluded as the clock | proved | 1:00 | not re-measured |
| `gate_decide_is_total` (oa-track) | tracker clock gate (`gate_decide`): no panic, overflow or out-of-bounds access for any clock (`i64::MIN` too), time, address, sensor and live-track count (OA-TRK-193) | six consecutive calls from an empty gate, which fill the 4-place sensor table and reuse a place; any configuration with `0 <= ooo <= limit < drop <= 1 day` | proved | 1:22 | 1.5 GB |
| `gate_accept_is_within_window_and_limit` | an `Accept` is never more than the step limit ahead of the clock nor older than the out-of-order window (OA-TRK-198) | one call (the verdict does not depend on the runs), clock and time within 2^61 ns of zero | proved | 0:08 | 302 MB |
| `gate_step_forward_needs_a_confirmed_run` | with live tracks, a far-ahead measurement is held or steps; it steps at once when only its own track and sensor are live (`alone`); otherwise fewer than three in a row never steps and the third steps only if the three come from at least min(2, live sensors) distinct sensors and min(3, live tracks) distinct addresses; `kani::cover!` that a lone step and a confirmed step are each reachable (OA-TRK-194) | three far-ahead measurements, nondecreasing, any addresses and sensors, live sensors, live tracks (above 0) and the `alone` flag any value, time within 2^61 | proved; cover satisfied | 0:58 | 465 MB |
| `gate_step_back_needs_span_count_and_spread` | fewer than six late refusals never step back; the sixth steps only if the run spans at least the window, to the newest time of the run, from at least min(2, live back-counted sensors) sensors (those whose newest accepted time is no later than the run's newest plus the window) and min(3, live tracks) addresses; `kani::cover!` that a step back is reachable (OA-TRK-195) | six late measurements, any addresses and sensors, any live counts, time within 2^61 | proved; cover satisfied | 1:07 | 1.1 GB |
| `resolve_decide_needs_apart_reports_to_confirm_and_two_to_roll_back` | the pure function `resolve_decide` only (OA-TRK-348, 352, 353): a report under 0.5 s after the move decides nothing; `Confirm`, `Rollback` and the first old-path report (`OldVote`) only for a report at least 0.5 s after the move and within the window, within the agreement limit, which never passes `confirm_move_m`; `Confirm` needs the old path no nearer and, with a snapshot, predictable; `Rollback` needs an earlier old-path report at least 0.5 s before it (one report never rolls back); an unpredictable old path decides nothing (`OldUnpredictable`); a tie confirms; `kani::cover!` that each outcome and a tie are reachable. Not proved: that `update_in_order_held` acts on the verdict as described, nor that the real filter's applied report decides this way (integration tests) | any accepted configuration, any window from the configured floor up, any `i64` times and vote time, any `f64` distances, a snapshot present or not | proved (local, 2026-11-20, `5e2d601`) | 0:45 | not re-measured |
| `update_in_order_with_a_large_move_starts_it_and_emit_marks_the_row_it_makes` | production `update_in_order` given a large-move decision (`decide` stubbed to `Moved`) starts the unconfirmed move (the harness does not set it) and the `emit` it calls marks the row it makes, with `filter_output` returning for the snapshot's filter a row at another position or none, and `filter_row_distance_m` and `r95_m` returning any value: the row is flagged; a row whose widening cannot be computed has the fail-closed covariance (500 km) and is counted; covers: widening fails, row widened over a differing old path, row already wide enough. Not covered (integration tests only): `update_in_order_held`, tick rows, the forecast, the rows a late replay emits, the numeric widened radius | any `i64` row time, any distance (not negative, or not a number), any non-negative radius | proved (local, 2026-11-20, `5e2d601`) | 1:39 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_scalars_and_counters` | production `update_in_order_held` with `decide` and `filter_apply` stubbed to what the real rejected path does and `filter_miss_m` returning any distance (so every verdict of `resolve_decide`), given a report that is not applied (OA-TRK-354): the move is still active with the same snapshot index, no outcome is counted, no row is made; `jump_rejected` is one more and at most one of `moves_decision_rejected` and `moves_old_path_votes` one more, and `moves_old_path_unpredictable` never (a refused report is counted as a refusal) (the vote also sets the old-path vote time to the report's). Nine harnesses over eight groups of columns (the move's own columns and the counters, which two harnesses take; scalar columns; first and confirmation times with the move's first report; state and counts; category; current filter; anchor filter; ring): in each the moved track differs from the snapshot in every group by one fixed value and exactly one group is symbolic, because a proof with every group symbolic at once does not fit the memory budget. Symbolic in all nine: the move's start time, the old-path vote time (or none), the report's time (a 1000 ns window after the newest update) and flags, the stubbed distances. Symbolic in one only: the last-update time and update count; the track's first and confirmation times and the move's first report time and sensor (`mv_first_ns`, `mv_first_sens`; fixed in the others, and not made symbolic there again without a run to show it fits); the state (confirmed or tentative), hit count, ring length and ring head; the category; the current filter (run of refusals below `RESYNC_AFTER`); the anchor filter; in the ring harness only the first ring entry (`ring_m[0]`). Everything else is at one fixed value, including `disagree_until_ns`, `expected_ns` and `newest_update_ns` (checked unchanged at that default only) and the rest of the report. `decide`, `filter_apply` and `filter_miss_m` are stubs. Loops over the ring are bounded by the constant `RING_DEPTH` (12), no unwind bound of the harness's own. Afterwards the snapshot in the pool equals the one taken (field for field, bit for bit), the move's start, first report time and sensor are as before, the hold flag is off. They hold for the default configuration (`resync_consistent_k` 0, run of refusals below `RESYNC_AFTER`; the resynchronising refusal is an integration test, OA-TRK-352). Not covered (integration tests only): a report that is applied or is itself a large move, ticks, forecasts, the late replay. The eight siblings below compare the other counters, the last-update time and update count, the first and confirmation times, the state and counts, the category, the current filter, the anchor filter and the ring | any move start time, any vote time or none, any report time in the window, any distance | proved (local, 2026-11-20, `007a8a4`) | 6:03 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_other_counters` | the same scenario: every counter other than `jump_rejected` and the three move counters is as before | as above | proved (local, 2026-11-20, `5e2d601`) | 3:06 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_last_time_and_updates` | the same scenario: the last-update time, the update count (any values), the fusion layer's two columns and the newest-update bound are as before | as above | proved (local, 2026-11-20, `5e2d601`) | 5:41 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_first_times` | the same scenario: the track's first and confirmation times and the move's first report time and sensor (any values) are as before | as above | proved (local, 2026-11-20, `5e2d601`) | 6:17 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_state_and_counts` | the same scenario: the state, hit count, ring length and ring head (any values) are as before | as above | proved (local, 2026-11-20, `5e2d601`) | 5:58 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_category` | the same scenario: the category state (any value) is as before | as above | proved (local, 2026-11-20, `5e2d601`) | 4:59 | not re-measured |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_current_filter` | the same scenario: the current filter's vertical filter, trigonometry and every scalar (any value, run of refusals below the resynchronising one) are as before but for its run of refusals (one more), bit for bit | as above | proved (local, 2026-11-20, branch `lane-kani-mem`) | 2:51 | 4.5 GB |
| `update_in_order_held_with_a_report_that_is_not_applied_keeps_the_horizontal_current_filter` | the same scenario, current filter symbolic: its horizontal half (both models' state, covariance and weight) is as before, bit for bit | as above | proved (local, 2026-11-20, branch `lane-kani-mem`) | 2:38 | 3.4 GB |
| `update_in_order_held_with_a_report_that_is_not_applied_keeps_the_snapshot_with_any_current_filter` | the same scenario, current filter symbolic: the snapshot in the pool is unchanged, bit for bit | as above | proved (local, 2026-11-20, branch `lane-kani-mem`) | 5:03 | 12.1 GB |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_anchor_filter` | the same scenario: the anchor filter (any value) is as before, bit for bit | as above | proved (local, 2026-11-20, branch `lane-kani-mem`) | 3:25 | 6.3 GB |
| `update_in_order_held_with_a_report_that_is_not_applied_keeps_the_snapshot_with_any_anchor_filter` | the same scenario, anchor filter symbolic: the snapshot in the pool is unchanged, bit for bit | as above | proved (local, 2026-11-20, branch `lane-kani-mem`) | 5:07 | 12.1 GB |
| `update_in_order_held_with_a_report_that_is_not_applied_changes_only_the_refusals_ring` | the same scenario: the ring is as before, bit for bit | as above | proved (local, 2026-11-20, `5e2d601`) | 6:48 | not re-measured |
| `snap_restore_from_the_moves_snapshot_and_end_move_put_back_the_listed_columns` | `snap_restore` from the move's snapshot (a `track_txn` capture) and `end_move`, not the tracker's rollback as a whole (OA-TRK-349): every scalar column, the category, and the current filter's time and first state value are as before the move; `disagree_until_ns` and `expected_ns` (written by the fusion layer) and the newest-update bound are left as they are; the move ends counted and its snapshot returns to the pool. The whole filters and the ring: the three round-trip rows, of the same function (`snap_restore`), not the OA-TRK-335 proofs, which are of `restore_track_txn` | arbitrary scalars, category, filter time and first state value before the move, all changed by the move | proved (local, 2026-11-20, `5e2d601`) | 0:56 | not re-measured |
| `the_capture_snapshot_equals_the_snapshot_taken_scalars_and_category` | `snap_of_txn(&track_txn(t))` (the move's rollback point, from the update's own capture) equals `snap_take(t)` in every field, compared bit for bit, with the scalar columns and category symbolic; the four proofs together cover every field (OA-TRK-354), and a unit test compares a real track with a wrapped ring | any value of the group | proved (local, 2026-11-20, `5e2d601`) | 2:24 | not re-measured |
| `the_capture_snapshot_equals_the_snapshot_taken_current_filter` | `snap_of_txn(&track_txn(t))` (the move's rollback point, from the update's own capture) equals `snap_take(t)` in every field, compared bit for bit, with the current filter symbolic; the four proofs together cover every field (OA-TRK-354), and a unit test compares a real track with a wrapped ring | any value of the group | proved (local, 2026-11-20, `5e2d601`) | 2:50 | not re-measured |
| `the_capture_snapshot_equals_the_snapshot_taken_anchor_filter` | `snap_of_txn(&track_txn(t))` (the move's rollback point, from the update's own capture) equals `snap_take(t)` in every field, compared bit for bit, with the anchor filter symbolic; the four proofs together cover every field (OA-TRK-354), and a unit test compares a real track with a wrapped ring | any value of the group | proved (local, 2026-11-20, `5e2d601`) | 3:07 | not re-measured |
| `the_capture_snapshot_equals_the_snapshot_taken_ring` | `snap_of_txn(&track_txn(t))` (the move's rollback point, from the update's own capture) equals `snap_take(t)` in every field, compared bit for bit, with the ring symbolic; the four proofs together cover every field (OA-TRK-354), and a unit test compares a real track with a wrapped ring | any value of the group | proved (local, 2026-11-20, `5e2d601`) | 2:47 | not re-measured |
| `taking_then_restoring_a_snapshot_restores_the_current_filter` | `snap_take` then `snap_restore` (OA-TRK-349) restores the current filter bit for bit, every field of it symbolic; one harness for each so that each fits the memory budget (the scalar columns and the category: `snap_restore_from_the_moves_snapshot_and_end_move_put_back_the_listed_columns`) | any value of the current filter | proved (local, 2026-11-20, `5e2d601`) | 1:20 | not re-measured |
| `taking_then_restoring_a_snapshot_restores_the_anchor_filter` | `snap_take` then `snap_restore` (OA-TRK-349) restores the anchor filter bit for bit, every field of it symbolic; one harness for each so that each fits the memory budget (the scalar columns and the category: `snap_restore_from_the_moves_snapshot_and_end_move_put_back_the_listed_columns`) | any value of the anchor filter | proved (local, 2026-11-20, `5e2d601`) | 1:21 | not re-measured |
| `taking_then_restoring_a_snapshot_restores_the_ring` | `snap_take` then `snap_restore` (OA-TRK-349) restores the ring bit for bit, every field of it symbolic; one harness for each so that each fits the memory budget (the scalar columns and the category: `snap_restore_from_the_moves_snapshot_and_end_move_put_back_the_listed_columns`) | any value of the ring | proved (local, 2026-11-20, `5e2d601`) | 1:08 | not re-measured |
| `fixed_map_agrees_with_a_reference_model_2_slots` (oa-track) | `FixedMap` agrees with a reference list through any mix of insert, get, remove and retain (a symbolic predicate): `len`, `is_empty`, `refused`, every held pair found with its value, `iter` yields exactly the held pairs once each, an insert is refused exactly when the key is new and the table is full, a probe key is found exactly when held; no panic, overflow or out-of-bounds access (OA-TRK-307) | 2 slots, 4 operations, any `u32` key and `u8` value; the hash replaced by the identity (`-Z stubbing`, see Stubs) | proved | 8:04 | not re-measured |
| `quadrant_signs_follow_the_quadrant_table` (oa-math) | `sin_sign` and `cos_sign` equal the quadrant table (OA-MATH-030) | every quadrant 0 to 3, every sign bit | proved | 0:01 | not re-measured |
| `magic_number_rounds_half_even` | `magic_int(v + ROUND_MAGIC)` is `v` rounded to an integer, ties to even, and `t - ROUND_MAGIC` is the same integer as a double (OA-MATH-031) | every finite `v` with magnitude below 2^51 | proved | 0:14 | not re-measured |
| `cut_split_halves_are_exact_and_short` | the split of `two_prod_cut` (`cut27`): high half of at most 26 bits, low half below 2^27, each exact, sum equal to the input (OA-MATH-032) | every double with exponent field 53 to 2046 (normal, low half normal) | proved | 0:03 | not re-measured |
| `clip_row_count_refuses_exactly_when_the_total_crosses_the_cap` (oa-truth) | the clip reader's row accumulation (`claim_rows`) cannot overflow and refuses at the first frame whose exact running total is over the cap, and at no other (OA-TRUTH-046) | 3 frames, any `u32` claim each, any cap | proved | 0:03 | not re-measured |
| `clip_claim_rows_is_total` | `claim_rows` does not panic for any running count, including near `u64::MAX`; the count never goes down | any `u64`, `u32`, `u64` | proved | 0:02 | not re-measured |

The current-filter and anchor-filter cases were each one harness until 2026-11-20. Together with the snapshot check they peaked at 13.7 and 13.8 GB, and a 16 GB CI runner ran out of memory on the first (allocation failure in Kani's driver). Each is now two harnesses, the filter and the snapshot, which together assert exactly what the one harness did; the current filter's check is further split into its horizontal half and the rest. The snapshot check is the costly part (12.1 GB alone). The suite refuses a registry in which one harness name contains another, because Kani's `--harness` matches by substring and would select both.


### Bounds and how they were chosen

A harness takes the largest bound the solver finished in a few minutes inside a memory limit.
Larger ones were tried and did not finish (see below). The bound is part of the result: for a
longer input the proofs say nothing.

Assumptions a harness makes about state are stated in the harness and are:

* `clock_observe`: each feed's `cand_n <= CLOCK_CONFIRM_FRAMES` (the function keeps it so: a refused step leaves the count at the limit and a continuing candidate does not raise it; a state with `cand_n`
  at `u32::MAX` would overflow the increment, and the type does not prevent one being built), and
  the counters `held`, `forward_steps`, `backward_steps`, `steps_outvoted` below `u64::MAX` (2^64 frames).
  at `u32::MAX` would overflow the increment, and the type does not prevent one being built), and
  the counters `held`, `forward_steps`, `backward_steps` below `u64::MAX` (2^64 frames).
* `observe_seq`: `duplicates`, `regressions` and `reordered` below `u64::MAX`; the open gap (start and length) lies below the next expected number and within `missing`, which is at most the sum of the gap lengths added so far.
* The clock gate harnesses: the configuration satisfies the configuration check (window not
  negative, limit at least the window, drop age above the limit and at most one day); times are
  within 2^61 ns of zero except in the totality harness, which takes every `i64`. The sensor table
  has 4 places and a run buffer holds 6 pairs under Kani (64 and 8 in the build), so a proof of
  the table being full and of the late run that can step back fits; the logic is the same
  code. The step rules are proved on the run decision with the live-sensor count as a free input,
  so they hold for any table content; the table is covered by the totality proof and the tests.
  Loops in the gate are plain index loops: iterator chains and computed buffer bounds made the
  same proofs run out of 6 GB.

### Found by the proofs

* A frame time equal to `i64::MIN`, the clock's "no frame yet" value, could complete a backward
  step to it in three frames, after which the next frame of any time was taken as a first frame
  and accepted with no confirmation (the consistency check of
  `clock_advances_by_at_most_the_tolerance_unless_the_feed_is_alone` (then named `clock_moves_forward_unconfirmed_by_at_most_the_tolerance`) failed on it). Such a time is now held
  and counted, never taken as the clock (OA-RPL-081).

### Stubs

`payload_records_*` replace two functions by contracts so that the solver does not redo them for
every record. `decode_row` becomes "returns nothing, or a row with anomaly bits among the three
that exist", which is what `decode_row_total_ads2` and `decode_row_total_ads1` prove of the real
function. `reports_push`, thirty `Vec` appends, becomes a no-op, so the output columns are not
inspected by those two harnesses. The real `decode_row` is therefore proved on its own for every
record, and the loop and the counting with the stub, which is sound where the stub's behaviour
covers the real function's (it does: any record may be dropped or kept).

`fixed_map_agrees_with_a_reference_model_2_slots` replaces `mix` by the identity. The table reads a
key only through equality and its home slot, and with free keys every assignment of homes is
already reachable, so the real mixer (64-bit multiplications) costs time and adds no case; the mixer
itself is a bijection tested by the unit tests, not proved.

### What did not finish

* `decode_block` and the typed decoders (`decode_item` of CAT021, CAT048, CAT062) over arbitrary
  bytes. `decode_block` over any category byte with 3 octets ran out of the 6 GB limit; with a
  fixed category, `parse_block` at 6 octets (the first size that holds a record) and `decode_block`
  at 8 and 10 octets each ran past 8 minutes (limit 10) without finishing; the typed `decode_item`
  loop over all FRNs of CAT048 for 12 octets ran past 7 minutes and 5 GB. The cause is a
  symbolic index into the UAP tables together with `Vec` growth under symbolic conditions. What is
  proved instead is the framing, the FSPEC, the walk of every item (which is where length and
  offset arithmetic lives) and the primitives, each separately, to the bounds above.
* The round trip `encode_block(decode_block(x)) == x` and `decode(encode(decode(x))) == decode(x)`
  for whole blocks, which needs `decode_block`. What is proved are the round trips of the parts the
  codec's conversions rely on (identification, time of day). The whole-block round trip stays tested
  (OA-AST-011, OA-AST-015, OA-AST-016, OA-AST-023) and fuzzed, not proved.
* The position conversion `e7_to_raw(raw_to_e7(raw)) == raw` for 23, 25 and 30 bits (64-bit
  division in a bit-blasted solver) ran past 5 minutes for 23 bits and was dropped; it stays a test
  (OA-AST-013).
* `FixedMap` over four slots with three operations, and over two slots with six operations, ran
  past 9 minutes (limit 10) without finishing (symbolic `Vec` of slots with nested probe loops); what
  is proved is two slots, four operations.
* `check_split` (oa-bench) for symbolic sets of 2 rows each (three each, and a symbolic count of 0
  to 2, were also tried): the key comparison of 14 columns (a 96-byte `memcmp`) and the 30-column
  `Reports` pushes did not finish within 10 minutes. It stays tested.
* A symbolic ADS-B payload with more than 4 records, and the real (unstubbed) record decoder inside
  the payload loop: 1 record with everything real ran past 15 minutes and 4.5 GB.

### Conflict alert: what is proved and what is tested in `predict`

`oa_ca::solve::predict(...)` returns `None` or a prediction whose time to loss is finite and inside
the look-ahead and whose two misses are finite and not negative (OA-CA-029). `predict` is exactly
`conflict_span`, then `horizontal_miss` and `vertical_miss`, then `prediction_of`. The split equals
the unsplit code bit for bit (`crates/oa-ca/tests/split.rs`). Constants: `A_MAX = 3.3e7`,
`DISC_MAX = 4e19`, `TC_MAX = 4e18`, `ROOT_MAX = 7e9`, `POINT_MAX = 5e7`. Harnesses are in
`crates/oa-ca/src/solve.rs` (`solve::proofs`); the ids are those of `tools/rust_proofs.json`.

| Contract | Established by | Used by |
|---|---|---|
| `a = vx^2 + vy^2` in `[0, A_MAX]`; discriminant finite and `<= DISC_MAX` | proved, `ca-horizontal-now-bounds` | the decision of the first step |
| time of closest approach finite, `<= TC_MAX`, for a speed not taken as zero | proved, `ca-horizontal-now-approach` | the first step, `ca-horizontal-now-root` |
| `sqrt(disc)` finite, `0 <= root <= ROOT_MAX`, for finite `disc` in `(0, DISC_MAX]` | proved, `ca-horizontal-now-sqrt` | `ca-horizontal-now-root` |
| a pair inside the limit now: `step_of` gives a current span or a root step with `disc > 0` | tested: edge margins and 40,000 random inputs | `ca-horizontal-now-root` (OA-CA-028) |
| the horizontal span is finite and ordered inside the look-ahead | proved, `ca-horizontal-span` (whole solver) | `ca-predict-span` (as the ordered span it takes) |
| the vertical span is finite and ordered; the vertical miss is finite, not negative | proved, `ca-vertical-span` | `ca-predict-span`, `prediction_of` |
| the conflict span, through `meet`, is finite and ordered inside the look-ahead | proved, `ca-predict-span` | `prediction_of` |
| the closest point is finite, within `POINT_MAX`, at a time in `[0, t]` | proved, `ca-horizontal-miss-point` | the horizontal miss |
| the horizontal miss, the exactly rounded root of `x*x + y*y`, is finite and not negative | proved, `ca-horizontal-miss`, on the real function | `prediction_of` |
| time to loss finite and in `[0, t]`; misses carried unchanged | tested: edge cases and 40,000 random spans and misses | the end-to-end property |

So the end-to-end property is a composition of proofs with two tested links, the first step's
decision and `prediction_of`. Both are short branching functions that the sweeps cover over their
whole domain. The miss rests on the exact rounding of `oa_ieee::sqrt` and on no other library
contract. The crate no longer calls `oa_math::hypot`; its agreement with `hypot` is tested
(`tests/miss.rs`).

### Conflict alert: the grid's pair test (OA-CA-033, OA-CA-034)

Nothing in the pair test is proved. The squared chord of two vectors with components in `[-1, 1]` is
finite, not negative, symmetric and zero for one point, and the flat test's validity only narrows as a
tangent or the radius grows. That the chord is never longer than the arc, and that a valid flat test
accepts every pair the exact projection does, rest on the trigonometry of `oa_math`. All of these are
dense deterministic tests (`grid::tests`, `tests/ca.rs`).

## What the proofs do not cover

* **Inputs beyond the bound.** A 25-octet item, a 4-octet CAT021 FSPEC, a 5-record ADS-B payload
  are outside every claim above.
* **Floating point and accuracy.** The proofs are about integer and byte logic. Nothing here says a
  position, speed or time is accurate; the benchmark and truth tests do that.
* **Logic not stated as a property.** A function proved not to panic can still compute the wrong
  value. Only the assertions in each harness are checked.
* **The compiler, the standard library, CBMC and Kani.** The proofs are about the Rust code as
  Kani models it. Kani's models of `Vec`, slices and integer operations are trusted. The
  toolchain Kani builds with is not the one the shipped binary is built with.
* **The two stubs above** (they are assumed, and each is backed by its own proof or by being
  an append).
* **The assumed state** listed above.
* **Concurrency, I/O, timing.** None of it is in a harness.

## Certification

Formal methods can supply evidence in a ground ATM software assurance case: RTCA DO-278A, the
guidance for communications, navigation, surveillance and air traffic management software, is
supplemented by DO-333, which sets out how formal methods may be used and what evidence they
give. This document is a description of what was checked, not a claim that OpenATCA meets or is
certified to any standard. A real use would need the soundness of the tool to be argued, the
requirements to be the ones the system is assessed against, and the proofs to be repeatable from
a controlled environment.

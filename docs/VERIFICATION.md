# Formal verification

This is the first set of machine-checked proofs in OpenATCA. It is narrow on purpose: each proof
states a property, the bound it holds up to, and what it does not say. Nothing here is a
certification claim.

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
`crates/oa-adsb/src/frames.rs`, `crates/oa-feed/src/health.rs` and
`crates/oa-replay/src/clock.rs` and `crates/oa-track/src/time_gate.rs` (each at the end, in a `proofs`
module).

Each harness is tagged `// REQ: OA-...-1nn` directly above `#[kani::proof]`. `oa-trace` counts that
as verification of the requirement and lists it as a proof, apart from tests.

## How to run

Kani 0.68.0 (CBMC 6.11.0) must be installed (`cargo install --locked kani-verifier`, then
`cargo kani setup`). One harness at a time:

```
cargo kani -p oa-asterix --harness fspec_cat048_5
cargo kani -p oa-replay  --harness clock_observe_total_and_confirms
cargo kani -p oa-feed    --harness observe_seq_accumulation_saturates
cargo kani -p oa-adsb    --harness decode_row_total_ads2
cargo kani -p oa-adsb    --harness payload_records_ads2_4 -Z stubbing
cargo kani -p oa-track   --harness gate_step_back_needs_span_count_and_spread
```

The two `payload_records_*` harnesses use `#[kani::stub]` and need `-Z stubbing`. Several
harnesses take minutes and up to 5 GB; run them one at a time and under a memory limit. Kani builds
into `target/`.

## Results

Run on one laptop, single job, CBMC 6.11.0. Time is wall clock including compilation; memory is the
peak resident size of the run. Every harness below verified successfully (no counterexample).

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
| `payload_records_ads2_4` | `decode_payload_lenient` and `decode_payload`: no panic or overflow, kept + dropped = records, each anomaly count at most kept, strict reports only a record below the count | valid header, exactly 4 records, `ADS2`; `decode_row` and `reports_push` stubbed (see below) | proved | 3:32 | 3.2 GB |
| `payload_records_ads1_4` | same | same, `ADS1` (64-octet records) | proved | 2:49 | 3.2 GB |
| `observe_seq_accumulation_saturates` (oa-feed) | `observe_seq`: no panic or overflow; a forward jump adds one gap and the saturated missing count, anything else changes neither | every `seq`, every `frames`, `seq_next`, `seq_last`, `gaps`, `missing`; `duplicates` and `regressions` below `u64::MAX` | proved | 0:05 | 208 MB |
| `clock_observe_total_and_confirms` (oa-replay) | `clock_observe` over the per-feed clock: no panic or overflow for any feed index, `now` and `t`; a step carries the saturated difference, comes only from a candidate count of at least `CLOCK_CONFIRM_FRAMES - 1` in the frame's own feed, clears every count, and none comes from an empty candidate state; a frame never adds candidates to another feed | any `usize` feed, every `i64` pair, five feeds each with `cand_n` below `CLOCK_CONFIRM_FRAMES`, counters below `u64::MAX` | proved | 0:24 | 266 MB |
| `clock_moves_forward_unconfirmed_by_at_most_the_tolerance` | from a state the pipeline keeps (every feed with an accepted frame has it at or before the clock), one frame that does not complete a confirmation moves a running clock forward by at most `CLOCK_TOLERANCE_NS`, and the state stays consistent with the resulting clock | same, plus that state | proved | 0:12 | 231 MB |
| `clock_observe_three_candidates_before_a_step` | from an empty candidate state of one feed, no step on the first two frames; a step on the third only after two held frames, each within `CLOCK_TOLERANCE_NS` of the one before and on one side of the clock | every three frame times, clock fixed, `i64::MIN` excluded as the clock | proved | 0:24 | 578 MB |
| `gate_decide_is_total` (oa-track) | tracker clock gate (`gate_decide`): no panic, overflow or out-of-bounds access for any clock (`i64::MIN` too), time, address, sensor and live-track count (OA-TRK-193) | six consecutive calls from an empty gate, which fill the 4-place sensor table and reuse a place; any configuration with `0 <= ooo <= limit < drop <= 1 day` | proved | 1:15 | 1.5 GB |
| `gate_accept_is_within_window_and_limit` | an `Accept` is never more than the step limit ahead of the clock nor older than the out-of-order window (OA-TRK-198) | one call (the verdict does not depend on the runs), clock and time within 2^61 ns of zero | proved | 0:10 | 297 MB |
| `gate_step_forward_needs_a_confirmed_run` | with live tracks, a far-ahead measurement is held or steps; fewer than three in a row never steps; the third steps only if the three come from at least min(2, live sensors) distinct sensors and min(3, live tracks) distinct addresses; `kani::cover!` that a step is reachable (OA-TRK-194) | three far-ahead measurements, nondecreasing, any addresses and sensors, live sensors and live tracks any count (tracks above 0), time within 2^61 | proved; cover satisfied | 0:31 | 459 MB |
| `gate_step_back_needs_span_count_and_spread` | fewer than six late refusals never step back; the sixth steps only if the run spans at least the window, to the newest time of the run, from at least min(2, live sensors) sensors and min(3, live tracks) addresses; `kani::cover!` that a step back is reachable (OA-TRK-195) | six late measurements, any addresses and sensors, any live counts, time within 2^61 | proved; cover satisfied | 1:03 | 1.1 GB |

### Bounds and how they were chosen

A harness takes the largest bound the solver finished in a few minutes inside a memory limit.
Larger ones were tried and did not finish (see below). The bound is part of the result: for a
longer input the proofs say nothing.

Assumptions a harness makes about state are stated in the harness and are:

* `clock_observe`: each feed's `cand_n < CLOCK_CONFIRM_FRAMES` (the function keeps it so; a state with `cand_n`
  at `u32::MAX` would overflow the increment, and the type does not prevent one being built), and
  the counters `held`, `forward_steps`, `backward_steps` below `u64::MAX` (2^64 frames).
* `observe_seq`: `duplicates` and `regressions` below `u64::MAX`.
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
  `clock_moves_forward_unconfirmed_by_at_most_the_tolerance` failed on it). Such a time is now held
  and counted, never taken as the clock (OA-RPL-081).

### Stubs

`payload_records_*` replace two functions by contracts so that the solver does not redo them for
every record. `decode_row` becomes "returns nothing, or a row with anomaly bits among the three
that exist", which is what `decode_row_total_ads2` and `decode_row_total_ads1` prove of the real
function. `reports_push`, thirty `Vec` appends, becomes a no-op, so the output columns are not
inspected by those two harnesses. The real `decode_row` is therefore proved on its own for every
record, and the loop and the counting with the stub, which is sound where the stub's behaviour
covers the real function's (it does: any record may be dropped or kept).

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
* A symbolic ADS-B payload with more than 4 records, and the real (unstubbed) record decoder inside
  the payload loop: 1 record with everything real ran past 15 minutes and 4.5 GB.

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

# Display allocation check

A standalone validation binary for RULESET C5 over the display core: after every table is built, a full
traffic frame allocates nothing. It is excluded from the product workspace and links `oa-display`,
`oa-scope-replay` (and, for the fixture, `oa-airspace`, `oa-geo`, `oa-track`) by local path only. It adds no
external dependency and no code to a product target. It follows `validation/oa-alloc-check` (the tracker's
check): the only unsafe code is `src/allocator.rs`, which forwards each allocation call to the system allocator
unchanged and counts it with atomics (RULESET A0); `main.rs` denies unsafe code.

## Run

```sh
cargo +1.89.0 run --release --locked --offline            # the check
cargo +1.89.0 run --release --locked --offline -- --controls-only
cargo +1.89.0 run --release --locked --offline -- --inject-allocation   # must exit 1
cargo +1.89.0 test --release --locked --offline           # the control unit test (OA-DSP-052)
```

Set `CARGO_TARGET_DIR` outside the checkout. The binary prints one JSON line. A zero exit and
`PASS_DSP_EXERCISED` need the positive controls, zero allocation calls in every window, and the exercised-path
evidence below.

## Windows (each 20 frames, a 1920 by 1080 view at 60 NM)

| window | what is measured |
|---|---|
| `convert` | `targets_at`: 200 aircraft's rows become targets (live and ended tracks, off-map, callsigns, a conflict) |
| `build_traffic` | `build_traffic` over a full set of 64 (coasting, uncertain, unknown 95% radius, reacquired, conflict, trail, leader), 5 refused at capacity and 1 malformed, into a cleared list |
| `replay_frame` | `replay_build` (map on an empty model, traffic, stamp) and `replay_raster` |
| `replay_frame_refusing` | the same with room for 8 targets, so the refusal notice is drawn |

The fixture, the set and the lists are all built before counting starts. After counting is disarmed the
evidence is checked: the states above must have been drawn and counted, so a window that drew nothing cannot
pass. `--inject-allocation` adds one allocation inside every window and must fail with
`injection_requested: true`.

## Limits

Finite fixtures are not an all-input proof. The check sees calls to Rust's global allocator that remain in the
generated program, not direct OS allocation, and not source-level allocations that the optimiser removed.
It is not wired into `tools/check_allocations.py` or CI (which cover the tracker harness); run it with the
commands above.

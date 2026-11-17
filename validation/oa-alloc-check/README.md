# M3 allocation check

This is a standalone validation binary for allocation behavior in the real `oa-track` tracker and radar/ADS-B fusion implementation. It is excluded from the product workspace and links `oa-track` only by local path. It adds no external dependency, product feature, test hook, or code to the product targets.

The only unsafe-code exception is `src/allocator.rs`, as named in RULESET A0. The crate denies unsafe code; that module locally allows it for the `GlobalAlloc` implementation and its positive controls. Each unsafe operation forwards a valid allocation/layout pair to `std::alloc::System`, with an adjacent safety comment. Review this file line by line when it changes.

The standalone manifest also denies the product's unwrap, expect, panic and indexing Clippy lints. Fixture access is checked before arming; only the test assertion has a local expect allowance under E3.

## Run

From the repository root, with Rust 1.89.0 installed:

```sh
python3 tools/check_allocations.py --target-dir /path/to/build-cache \
  --out /path/to/new-evidence-directory
```

Both paths must be outside the checkout. The runner uses one Cargo job and checks layout, format, clippy, build, tests, normal execution and injected-allocation execution. It retains commands, exit codes, source/binary fingerprints and output hashes in `result.json`. The committed lockfile belongs to this standalone package. The full candidate gate additionally requires a frozen, clean source revision.

For direct inspection from this directory, use `cargo +1.89.0 run --release --locked --offline`. Retain the exact command, source revision, Rust version, and harness source hash with the output.

The binary emits one JSON line after all measured windows have ended. A zero exit and `PASS_M3_EXERCISED` require the positive controls, required exercised-path evidence, and zero observed allocation calls in both measured windows. Any missing control, missing path evidence, counter overflow, or nonzero count fails the process.

`cargo run --release --offline -- --controls-only` runs only the three positive controls and emits `PASS_CONTROLS` when the standard allocation APIs hit the installed global hook. The `alloc` count is two because the realloc control first obtains a global-allocator pointer. This is also a negative-control mode: temporarily removing `#[global_allocator]` from `src/allocator.rs` must make this command fail. `cargo run --release --offline -- --inject-allocation` injects one standard global allocation into each armed core window and must exit 1 with a structured `FAIL_M3_ALLOCATION` JSON line and `injection_requested: true`, after fixture coverage succeeds. Unexpected core allocations in the normal run emit the same status with `injection_requested: false`. Every full summary includes allocator-counter `overflow` booleans for controls, tracker, and fusion.

## Measured windows and exercised routes

Startup construction and all fixtures happen before measurement. The single-threaded executable then runs a positive-control window through `std::alloc::alloc`, `alloc_zeroed`, `realloc`, and `dealloc`, verifying that each allocation counter increments through the installed global allocator. A `#[cfg(test)]` unit test carries REQ OA-TRK-327 and checks each allocation count is at least one, allowing for libtest background allocations. It then creates real `Tracker` and `Fusion` instances and preallocated output tables before enabling counts.

The tracker window exercises first-use updates, confirmation, out-of-order refilter, malformed-measurement refusal, track-capacity refusal, prediction, advance and flush. It records output deltas for advance and flush and requires one successful prediction, positive advance and flush row counts, and a nonzero ended-track count. The fusion window builds four addressed reference tracks over eight scans using `radar_geom` and production `plot_convert`, then exercises registration until a reference residual is used and published. Two distant anonymous targets each have four distinct scan observations: the first three confirm each local track, and the fourth associates to the existing tracks. A later quiet advance must emit rows, including coast rows. Stats, local-key output rows, registration state and radar-track records are checked after counting is disarmed; the required path evidence must be nonempty before zero allocations can pass.

Owned-collection shell helpers are not called while armed. In particular, `fusion_take_radar_tracks`, `tracker_live`, and buffer-snapshot helpers allocate their returned collections; the caller may drain records or inspect snapshots outside the core window. `fusion_radar_tracks`, registration state, statistics, and caller-owned output are read without cloning after the measured calls. Startup constructors (`tracker_new`, `fusion_new`, `output_new`) are also outside the window by design.

## Scope and limits

A `PASS_M3_EXERCISED` result covers these named, deterministic M3 paths in this executable and configuration. It is not a proof for every possible input, every crate in OpenATCA, all platforms, or shell work excluded above. Add a named fixture and positive evidence before claiming a new route is covered. The compiler's allocation-elision rules also mean the global hook observes allocator calls that remain in the generated program, not every source-level allocation expression; retain source review alongside this runtime evidence.

The allocator hook counts Rust global allocator calls to `alloc`, `alloc_zeroed`, and `realloc`, including short-lived allocations. It does not count deallocation, and it cannot see direct OS allocation mechanisms that bypass Rust's global allocator. The hook uses atomics only; it does not format, lock, access TLS, call callbacks, or allocate. This follows the [`GlobalAlloc` contract](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html), including its restrictions on unwinding and re-entry.

# `oa-demo-wasm`

This is the scalar-only WASM shell for the F2 browser host. It accepts no flight data or pointers
from JavaScript. For the exact `wasm32-unknown-unknown` target, `build.rs` requires an explicit
prepared OASD asset path, its expected whole-file SHA-256, and the reviewed 40-character source
commit. The asset is copied into Cargo's generated output only after its size and digest checks;
at reset the engine verifies the embedded bytes again and parses the OASD v1 framing and records.
Native 64-bit workspace checks compile this shell package without ABI exports and without
requiring or embedding a substitute asset; they do not stand in for the WASM target check.

The exported `f2_*` functions keep all scenario, tracker, workstation and display state in Rust.
`f2_frame` returns a checked packed `(linear-memory offset:u32, length:u32)` for the engine's
canonical OAF2 frame packet. The packet remains stable until the next successful engine operation.
The only pointer use is an integer address returned to the host; no pointer enters the WASM ABI.
On wasm32 the offset is a linear-memory address. In native 64-bit parity runs, an address that does
not fit u32 returns no frame; the implementation never truncates a native pointer.

The browser transcript replayer is `replay_trace.mjs`. It validates the manifest, OASD whole-file
digest, trace grammar and bounds, replays the scalar ABI in Node's built-in WebAssembly runtime, and
writes an exclusive F2RESULT1 output only after every status and full-packet digest matches.
It uses Node built-ins only and is source for coordinator-run parity checks; it has not been
executed in this worktree.

The coordinator's build wrapper must provide `OA_DEMO_SCENARIO_ASSET`,
`OA_DEMO_SCENARIO_ASSET_SHA256`, and `OA_F2_SOURCE_COMMIT`, then record the actual source commit,
asset SHA-256 and WASM SHA-256 in `demo/site/engine/build.json`. A source commit string alone is
not proof of a clean tree; the wrapper must separately verify the frozen checkout before/after
build. The 32-bit target requires the complete Rust engine/display closure to pass the assigned
WASM width audit before the build is eligible for use.

# F2 browser frame host

This is a small static host for the Rust-generated F2 display frame. Rust owns the scenario,
tracker/display state, command parser and interaction rules. JavaScript forwards bounded keyboard
and pointer events, requests explicit virtual-time advances, validates and paints the Rust frame
packet, and records a replayable input transcript. It does not create aircraft or infer flight
state.

The page refuses to start unless `engine/build.json` and `engine/oa-f2-engine.wasm` are present and
the manifest's binary SHA-256 matches the file. The manifest contract is `oa-f2-wasm-build-v1` and
includes `build_id` (equal to `wasm_sha256`), the exact source commit, `scenario_id`
(`ord-normal-push-v1`), `scenario_seed` as a 16-digit hexadecimal string, and the prepared OASD
scenario asset SHA-256. The
coordinator supplies these files only after the Rust engine and ABI are frozen.

`assets/openatca-mark.svg` is copied byte-for-byte from the existing 23 Nov demo preview asset at
`openatca-demo-preview/web/assets/openatca-mark.svg`; SHA-256
`c678b64cc55685b451859d895fa7eb46ed2e727d5e703ace9bdd1249a1294c10`.

## Provisional scalar ABI

The private host ABI is intentionally scalar-only:

```text
f2_reset(seed:u64) -> i32
f2_advance_to(virtual_ms:u64) -> i32
f2_key(hid_usage:u32, modifiers:u32, down:u32, repeat:u32) -> i32
f2_pointer(x:i32, y:i32, buttons:u32, kind:u32) -> i32
f2_frame() -> u64                 // high 32 bits: offset; low 32 bits: length
memory: WebAssembly.Memory
__data_end, __heap_base: WebAssembly.Global // immutable linker address markers
```

Each mutating call returns zero on success and a named nonzero refusal otherwise. Rust validates
the enums, input range, viewport and nondecreasing virtual time. The host retains no authoritative
state. Keyboard usages are USB HID usage IDs derived from `KeyboardEvent.code`; modifier bits are
Control=1, Alt=2, Shift=4, Meta=8. Pointer kinds are down=1, up=2, move=3, cancel=4; pointer
coordinates are integer pixels in the 1280x720 backing frame, and `buttons` is the browser's button
bitmask. The 5,000 ms step matches the synthetic report interval declared for the named scenario.
Play/pause uses `performance.now()` to pace the virtual clock at 1×, 2×, 4×, or 8× and sends a
Rust advance at each crossed 5,000 ms scenario boundary. One- and five-minute forward-jump controls
are also available; time never moves backward. The transcript stores the exact virtual times and
input order, not browser wall time or the selected playback rate. A keyboard or pointer event during
playback first advances Rust to its current whole-millisecond virtual time and records that advance
before forwarding the event. A native replay therefore receives the same engine inputs independent
of host timing.

The canonical frame packet v1 is a 24-byte little-endian header followed by exactly 1280x720
four-byte pixels, with no row padding:

| Offset | Field |
|---:|---|
| 0 | ASCII `OAF2` |
| 4 | `u16` version = 1 |
| 6 | `u16` format = 1 (`BGRX8_LE`, Rust rasterizer byte order B,G,R,unused) |
| 8 | `u32` width = 1280 |
| 12 | `u32` height = 720 |
| 16 | `u32` stride = width × 4 |
| 20 | `u32` pixel bytes = stride × height |
| 24 | Visible pixels, exactly as Rust rasterized them |

The JS host checks all lengths, dimensions, format and the returned WASM memory range before it
copies bytes. It reacquires the memory view for each frame. Painting converts B,G,R,unused to
Canvas RGBA; frame hashing covers the canonical packet unchanged.

The host streams the fidelity registry (1 MiB cap), build manifest (16 KiB cap), and WASM module
(64 MiB cap) before parsing or instantiation. These are input-byte caps; the WASM engine and Canvas
still use their separate bounded runtime/frame storage.

## Local use

After the coordinator places the verified WASM artifact and manifest in `engine/`, serve this
directory over a fresh loopback origin. The initial host has no service worker or offline cache;
the manifest and WASM are fetched without browser cache reuse. From the repository root:

```sh
python3 -m http.server 8000 --bind 127.0.0.1 --directory demo/site
```

Open `http://127.0.0.1:8000/`. A local HTTP origin is required for WebAssembly fetch. If this
origin was used by an older preview with a service worker, use a fresh port/origin or clear that
origin's site data before loading this host.

The host saves an `F2TRACE1` transcript of the exact ordered key, pointer and virtual-time calls,
their Rust status codes, and the canonical packet SHA-256 after reset and after every accepted or
refused input. It is an input record, not a flight-state record. The Rust `oa-demo-runner` consumes
the same trace and emits computed statuses and packet digests for comparison. Matching hashes
demonstrate frame determinism for that build and transcript only; they are not operational or
workstation-fidelity evidence. The source path is present, but parity has not been validated.

After a coordinator has prepared and pinned the OASD asset, built the WASM, and recorded the exact
source and asset bindings, the intended parity commands are:

```sh
node tools/oa-demo-wasm/replay_trace.mjs \
  --wasm <private-wasm-file> --manifest <private-build-manifest> \
  --scenario <private-oasd-file> --trace <private-f2trace-file> \
  --out <new-private-wasm-result>

cargo +1.89.0 run --locked -p oa-demo-engine --bin oa-demo-runner -- \
  --scenario <private-oasd-file> --trace <private-f2trace-file> \
  --source-commit <40-lowercase-hex-source-commit> --out <new-private-native-result>

cmp <private-wasm-result> <private-native-result>
```

These commands are source-level examples. They have not been run; the coordinator must first
freeze the engine source, prepare the public-data OASD bundle, and produce the matching build
manifest and transcript.

The bounded trace is UTF-8/ASCII with LF line endings and this record order:

```text
F2TRACE1
BUILD<TAB><64 lowercase hex wasm SHA-256>
SOURCE<TAB><40 lowercase hex source commit>
ASSET<TAB><64 lowercase hex prepared OASD SHA-256>
SCENARIO<TAB>ord-normal-push-v1
SEED<TAB>0x4F52442D4E505631
FRAME<TAB>0<TAB><64 lowercase hex canonical packet SHA-256>
<A, K, or P input record>
RESULT<TAB><signed i32 returned by Rust>
FRAME<TAB><1-based input index><TAB><64 lowercase hex packet SHA-256>
... repeated input/result/frame triples ...
```

`A` contains the exact nondecreasing virtual millisecond timestamp. `K` contains HID usage,
modifier bits, key-down flag and repeat flag. `P` contains backing-pixel x/y, browser button bits
and pointer kind. Keyboard/pointer inputs use the virtual time set by the most recent `A` record.
Browser wall timestamps and playback-rate choices are intentionally absent. A native replayer must
validate this grammar and its 4 MiB/16,384-event caps before applying any record.

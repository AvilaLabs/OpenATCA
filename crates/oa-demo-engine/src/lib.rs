#![forbid(unsafe_code)]
//! Deterministic demonstration engine. Pure state transitions live in `core`; artifact I/O and
//! decoding live in `shell` (RULESET B1, B7).
// B10: 64-bit native targets and the audited wasm32-unknown-unknown target are supported.
#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("oa-demo-engine supports 64-bit targets and audited wasm32-unknown-unknown only");

pub mod core;
pub mod shell;

pub use crate::core::{
    encode_frame_packet, normalize_host_key_modifiers, DemoEngine, DemoEngineConfig,
    EngineCounters, EngineError, FrameView, DEMO_FRAME_HEIGHT, DEMO_FRAME_STRIDE, DEMO_FRAME_WIDTH,
    FRAME_PACKET_FORMAT_BGRX8_LE, FRAME_PACKET_HEADER_LEN, MAX_AIRCRAFT, MAX_SCENARIO_REPORTS,
    NORMAL_SAMPLE_INTERVAL_MS, POINTER_CANCEL, POINTER_DOWN, POINTER_MOVE, POINTER_UP,
    SYNTHETIC_ADDRESS_BASE,
};
pub use crate::shell::from_oasd;

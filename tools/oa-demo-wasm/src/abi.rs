//! Bounded scalar-only FFI exports for the F2 demo host. This is the sole source file permitted
//! to contain C ABI export attributes; it uses no unsafe operation or pointer dereference.
#![allow(unsafe_code)] // RULESET A0: only the fixed f2_* extern names below use ABI export attributes.

use core::cell::RefCell;
use oa_demo_engine::{encode_frame_packet, from_oasd, normalize_host_key_modifiers, DemoEngine};

const BASELINE_SEED: u64 = 0x4F52_442D_4E50_5631;
const MAX_SCENARIO_MS: u64 = 3_600_000;
const FRAME_HEADER_BYTES: usize = 24;
const FRAME_PIXEL_BYTES: usize = 1280 * 720 * 4;
const FRAME_PACKET_BYTES: usize = FRAME_HEADER_BYTES + FRAME_PIXEL_BYTES;
const ABI_INPUT_ERROR: i32 = -10;
const ABI_STATE_ERROR: i32 = -127;

include!(concat!(env!("OUT_DIR"), "/asset_binding.rs"));
// seal-allow: build.rs embeds only the outer-prepared, seal-checked public synthetic OASD asset
static SCENARIO_OASD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scenario.oasd"));

struct Runtime {
    engine: DemoEngine,
    packet: Vec<u8>,
}

impl Runtime {
    fn new(seed: u64) -> Result<Self, i32> {
        if seed != BASELINE_SEED {
            return Err(ABI_INPUT_ERROR);
        }
        let mut packet = Vec::new();
        packet
            .try_reserve_exact(FRAME_PACKET_BYTES)
            .map_err(|_| ABI_STATE_ERROR)?;
        packet.resize(FRAME_PACKET_BYTES, 0);
        let mut runtime = Self {
            engine: from_oasd(SCENARIO_OASD, ASSET_SHA256).map_err(|error| error.status_code())?,
            packet,
        };
        runtime
            .engine
            .advance_to(0)
            .map_err(|error| error.status_code())?;
        runtime.refresh_frame()?;
        Ok(runtime)
    }

    fn refresh_frame(&mut self) -> Result<(), i32> {
        let frame = self
            .engine
            .render_frame()
            .map_err(|error| error.status_code())?;
        encode_frame_packet(&frame, &mut self.packet).map_err(|error| error.status_code())?;
        Ok(())
    }

    fn advance_to(&mut self, virtual_ms: u64) -> Result<(), i32> {
        if virtual_ms > MAX_SCENARIO_MS {
            return Err(ABI_INPUT_ERROR);
        }
        self.engine
            .advance_to(virtual_ms)
            .map_err(|error| error.status_code())?;
        self.refresh_frame()
    }

    fn handle_key(
        &mut self,
        hid_usage: u32,
        modifiers: u32,
        down: u32,
        repeat: u32,
    ) -> Result<(), i32> {
        if down > 1 || repeat > 1 || modifiers & !0x0f != 0 {
            return Err(ABI_INPUT_ERROR);
        }
        let engine_modifiers = normalize_host_key_modifiers(modifiers);
        self.engine
            .handle_key(hid_usage, engine_modifiers, down != 0, repeat != 0)
            .map_err(|error| error.status_code())?;
        self.refresh_frame()
    }

    fn handle_pointer(&mut self, x: i32, y: i32, buttons: u32, kind: u32) -> Result<(), i32> {
        if !(0..1280).contains(&x) || !(0..720).contains(&y) || !(1..=4).contains(&kind) {
            return Err(ABI_INPUT_ERROR);
        }
        let engine_kind = match kind {
            1 => 1, // down
            2 => 2, // up
            3 => 0, // move
            4 => 3, // cancel
            _ => return Err(ABI_INPUT_ERROR),
        };
        self.engine
            .handle_pointer(x, y, buttons, engine_kind)
            .map_err(|error| error.status_code())?;
        self.refresh_frame()
    }

    fn frame_range(&self) -> Option<u64> {
        // wasm32 offsets are linear-memory offsets. A native 64-bit process can have an address
        // above u32::MAX; that case deliberately reports no packet instead of truncating it.
        let offset = u32::try_from(self.packet.as_ptr() as usize).ok()?;
        let length = u32::try_from(self.packet.len()).ok()?;
        Some((u64::from(offset) << 32) | u64::from(length))
    }
}

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

fn with_runtime(f: impl FnOnce(&mut Runtime) -> Result<(), i32>) -> i32 {
    RUNTIME.with(|cell| match cell.try_borrow_mut() {
        Ok(mut state) => match state.as_mut() {
            Some(runtime) => f(runtime).map_or_else(|status| status, |_| 0),
            None => ABI_STATE_ERROR,
        },
        Err(_) => ABI_STATE_ERROR,
    })
}

#[no_mangle]
pub extern "C" fn f2_reset(seed: u64) -> i32 {
    RUNTIME.with(|cell| match cell.try_borrow_mut() {
        Ok(mut state) => match Runtime::new(seed) {
            Ok(runtime) => {
                *state = Some(runtime);
                0
            }
            Err(status) => status,
        },
        Err(_) => ABI_STATE_ERROR,
    })
}

#[no_mangle]
pub extern "C" fn f2_advance_to(virtual_ms: u64) -> i32 {
    with_runtime(|runtime| runtime.advance_to(virtual_ms))
}

#[no_mangle]
pub extern "C" fn f2_key(hid_usage: u32, modifiers: u32, down: u32, repeat: u32) -> i32 {
    with_runtime(|runtime| runtime.handle_key(hid_usage, modifiers, down, repeat))
}

#[no_mangle]
pub extern "C" fn f2_pointer(x: i32, y: i32, buttons: u32, kind: u32) -> i32 {
    with_runtime(|runtime| runtime.handle_pointer(x, y, buttons, kind))
}

#[no_mangle]
pub extern "C" fn f2_frame() -> u64 {
    RUNTIME.with(|cell| match cell.try_borrow() {
        Ok(state) => state.as_ref().and_then(Runtime::frame_range).unwrap_or(0),
        Err(_) => 0,
    })
}

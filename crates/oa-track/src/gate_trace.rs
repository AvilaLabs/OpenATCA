//! Experimental trace capture is unavailable in the pure tracker cut. The former global
//! thread-local collector was removed; a future diagnostic must pass a bounded sink explicitly.

/// One velocity-gate decision. Records are not produced until an explicit bounded sink exists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateTraceRow {
    pub addr: u32,
    pub toa_ns: i64,
    pub d2: f64,
    pub gated: bool,
}

/// Whether the explicit gate trace sink is available in this build.
pub fn gate_trace_available() -> bool {
    false
}

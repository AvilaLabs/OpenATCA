//! Diagnostic record of the velocity gate: with the `gate-trace` feature, every measurement the
//! filter applies for the first time is noted with the time, the address and the velocity
//! gate's distance and decision. Without the feature nothing is recorded and the take function
//! returns an empty list. Used by the `velocity_diag` example to say where gated velocities fall.

/// One applied measurement. Plain record (A1); A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateTraceRow {
    pub addr: u32,
    pub toa_ns: i64,
    /// The normalised squared velocity innovation (NaN when the measurement has no velocity or
    /// the distance is not a number).
    pub d2: f64,
    /// The velocity was dropped.
    pub gated: bool,
}

#[cfg(feature = "gate-trace")]
thread_local! {
    static TRACE: std::cell::RefCell<Vec<GateTraceRow>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "gate-trace")]
pub(crate) fn gate_trace_push(row: GateTraceRow) {
    TRACE.with(|t| t.borrow_mut().push(row));
}

#[cfg(not(feature = "gate-trace"))]
pub(crate) fn gate_trace_push(_row: GateTraceRow) {}

/// The rows recorded on this thread since the last call, in order.
pub fn gate_trace_take() -> Vec<GateTraceRow> {
    #[cfg(feature = "gate-trace")]
    {
        TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
    }
    #[cfg(not(feature = "gate-trace"))]
    {
        Vec::new()
    }
}

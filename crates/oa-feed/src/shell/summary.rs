//! The one-line text form of [`FeedHealth`]. Shell (B1, B7): it allocates a `String` per call, so
//! it is not real-time core; the figures it prints come from the pure [`crate::health`] code.

use std::fmt::Write;

use crate::health::{
    latency_quantile_ms, rates_milli, FeedHealth, LAT_OVERFLOW_MS, STATE_CONNECTED,
    STATE_DISCONNECTED,
};

/// One-line key=value summary. `now_ns` is the time of the report (live: the clock; replay:
/// `h.last_recv_ns`).
pub fn format_summary(h: &FeedHealth, now_ns: i64) -> String {
    let (r1, r60) = rates_milli(h, now_ns);
    let state = match h.state {
        STATE_CONNECTED => "connected",
        STATE_DISCONNECTED => "disconnected",
        _ => "unknown",
    };
    let since_ms = if h.frames == 0 {
        0
    } else {
        now_ns.saturating_sub(h.last_recv_ns).max(0) / 1_000_000
    };
    let size_min = if h.messages == 0 { 0 } else { h.size_min };
    let overflow = format!(">={LAT_OVERFLOW_MS}");
    let samples: u64 = h.lat_hist.iter().sum();
    let q = |n: u64, d: u64| match latency_quantile_ms(h, n, d) {
        _ if samples == 0 => "-".to_string(),
        // A bin's upper edge can exceed the largest sample seen; the quantile is never above
        // the maximum, so clamp to it.
        Some(e) => e.min(h.lat_max_ms).to_string(),
        None => overflow.clone(),
    };
    let lat_max = match h.lat_max_ms {
        _ if samples == 0 => "-".to_string(),
        m if m >= LAT_OVERFLOW_MS => overflow.clone(),
        m => m.to_string(),
    };
    let mut s = String::with_capacity(256);
    let _ = write!(
        s,
        "state={state} msgs={} rate_1s={}.{:03} rate_60s={}.{:03} bytes={} size_min={size_min} \
         size_max={} lat_p50_ms={} lat_p95_ms={} lat_max_ms={lat_max} skew={} gaps={} missing={} dups={} regress={} reordered={} \
         redelivered={} rejects={} reconnects={} errors={} skipped={} since_last_ms={since_ms}",
        h.messages,
        r1 / 1000,
        r1 % 1000,
        r60 / 1000,
        r60 % 1000,
        h.payload_bytes,
        h.size_max,
        q(1, 2),
        q(19, 20),
        h.skew,
        h.gaps,
        h.missing,
        h.duplicates,
        h.regressions,
        h.reordered,
        h.redelivered,
        h.rejects,
        h.reconnects,
        h.frame_errors,
        h.skipped_bytes,
    );
    s
}

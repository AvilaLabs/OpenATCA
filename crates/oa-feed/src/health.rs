//! Feed health (INTERFACES IF-R6): a plain record updated once per frame by free functions.
//! Core (pure): time is always an argument (B4), memory is fixed (C1, C2), and every figure is a
//! function of frame fields and the injected times, so a replay of a recording reproduces the
//! live figures (B6).
//!
//! Decisions where the specification leaves room:
//! * Rates count MESSAGE frames. The 1 s rate is the count in the last *complete* second; the
//!   60 s rate is the mean over the last up-to-60 complete seconds (divided by the seconds
//!   actually elapsed since the first frame, so a young feed is not under-reported). Rates are
//!   integers in milli-messages per second (A9).
//! * The ring holds 61 one-second slots, so the current partial second never overwrites the
//!   oldest second of the 60 s window. A slot carries its second as a tag; a stale slot reads
//!   as empty.
//! * Sequence: `seq` equal to the next expected is normal; above it is a gap (one gap event,
//!   `missing` counts the frames); equal to the last seen is a duplicate; any other lower value
//!   is a regression (reordering, or a bridge restart, which counts from 0 again), after which
//!   the expectation restarts from that value.
//! * Latency is `recv_unix_ns / 1e6 - jms_timestamp_ms` in whole milliseconds, for MESSAGE
//!   frames with a JMS timestamp; negative values are clock skew and are counted apart from the
//!   histogram.

use std::fmt::Write;

use crate::frame::{Frame, FrameError, Kind, FLAG_REDELIVERED};

/// One-second slots in the rate ring (60 complete seconds plus the current one).
pub const RING_SLOTS: usize = 61;
/// Payload-size histogram bins: bin `i` holds sizes whose bit length is `i` (bin 0: empty,
/// bin 1: 1 byte, bin 2: 2..3, ... bin 23: 4 MiB).
pub const SIZE_BINS: usize = 24;
/// Latency histogram bins (see [`LAT_EDGES_MS`]).
pub const LAT_BINS: usize = 14;
/// Upper edges (exclusive) of latency bins 0..13, in milliseconds; the last bin is open.
pub const LAT_EDGES_MS: [i64; LAT_BINS - 1] =
    [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000];

/// Connection state before any STATUS frame.
pub const STATE_UNKNOWN: u8 = 0;
/// Last STATUS said `state=connected`.
pub const STATE_CONNECTED: u8 = 1;
/// Last STATUS said `state=disconnected` or `state=reconnect_attempt`.
pub const STATE_DISCONNECTED: u8 = 2;

/// One second of traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Slot {
    /// The Unix second this slot holds.
    pub sec: i64,
    /// MESSAGE frames in that second.
    pub count: u32,
    /// Payload bytes in that second (saturating).
    pub bytes: u32,
}
// Budget: 8 + 4 + 4, no padding; 61 per feed, touched once per frame.
const _: () = assert!(core::mem::size_of::<Slot>() == 16);

/// All health figures for one feed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FeedHealth {
    /// Receive time of the first frame (ns), valid once `frames > 0`.
    pub first_recv_ns: i64,
    /// Receive time of the most recent frame (ns).
    pub last_recv_ns: i64,
    /// Frames of every kind accepted.
    pub frames: u64,
    /// MESSAGE frames.
    pub messages: u64,
    /// Payload bytes of MESSAGE frames.
    pub payload_bytes: u64,
    /// Next expected `seq`.
    pub seq_next: u64,
    /// Last `seq` seen.
    pub seq_last: u64,
    /// Gap events (`seq` jumped forward).
    pub gaps: u64,
    /// Frames missing across all gaps.
    pub missing: u64,
    /// Frames repeating the previous `seq`.
    pub duplicates: u64,
    /// Frames with a lower `seq` than the last (reorder or bridge restart).
    pub regressions: u64,
    /// MESSAGE frames flagged JMS-redelivered.
    pub redelivered: u64,
    /// REJECT frames.
    pub rejects: u64,
    /// `state=reconnect_attempt` STATUS frames.
    pub reconnects: u64,
    /// Delay announced by the latest reconnect attempt (ms).
    pub last_reconnect_delay_ms: u64,
    /// Latency samples below zero (clock skew), kept out of the histogram.
    pub skew: u64,
    /// MESSAGE frames with no JMS timestamp (no latency sample).
    pub no_timestamp: u64,
    /// Frames refused by the reader.
    pub frame_errors: u64,
    /// Bytes the reader discarded while resynchronising.
    pub skipped_bytes: u64,
    /// Smallest payload seen; `u32::MAX` until the first MESSAGE.
    pub size_min: u32,
    /// Largest payload seen.
    pub size_max: u32,
    /// Connection state ([`STATE_UNKNOWN`], [`STATE_CONNECTED`], [`STATE_DISCONNECTED`]).
    pub state: u8,
    /// Reserved, zero; keeps the layout free of implicit padding.
    pub reserved: [u8; 7],
    /// Payload-size histogram.
    pub size_hist: [u64; SIZE_BINS],
    /// Latency histogram.
    pub lat_hist: [u64; LAT_BINS],
    /// Per-second ring for the rates.
    pub ring: [Slot; RING_SLOTS],
}
// Budget: 19 u64/i64 fields (152) + 2 u32 (8) + state and reserved (8) + 24 * 8 + 14 * 8 + 61 * 16
// = 1448. One per feed, updated per frame; the rings dominate and have no padding.
const _: () = assert!(core::mem::size_of::<FeedHealth>() == 1448);

impl FeedHealth {
    /// An empty health record.
    pub const fn new() -> FeedHealth {
        FeedHealth {
            first_recv_ns: 0,
            last_recv_ns: 0,
            frames: 0,
            messages: 0,
            payload_bytes: 0,
            seq_next: 0,
            seq_last: 0,
            gaps: 0,
            missing: 0,
            duplicates: 0,
            regressions: 0,
            redelivered: 0,
            rejects: 0,
            reconnects: 0,
            last_reconnect_delay_ms: 0,
            skew: 0,
            no_timestamp: 0,
            frame_errors: 0,
            skipped_bytes: 0,
            size_min: u32::MAX,
            size_max: 0,
            state: STATE_UNKNOWN,
            reserved: [0; 7],
            size_hist: [0; SIZE_BINS],
            lat_hist: [0; LAT_BINS],
            ring: [Slot {
                sec: i64::MIN,
                count: 0,
                bytes: 0,
            }; RING_SLOTS],
        }
    }
}

impl Default for FeedHealth {
    fn default() -> Self {
        FeedHealth::new()
    }
}

const NS_PER_SEC: i64 = 1_000_000_000;

fn ring_index(sec: i64) -> usize {
    sec.rem_euclid(RING_SLOTS as i64) as usize
}

/// Folds one accepted frame into `h`. `now_ns` is the time the frame is observed (live: its
/// receive time; replay: the same field), never read from a clock here.
pub fn observe_frame(h: &mut FeedHealth, f: &Frame<'_>, now_ns: i64) {
    observe_seq(h, f.seq);
    if h.frames == 0 {
        h.first_recv_ns = now_ns;
    }
    h.frames += 1;
    h.last_recv_ns = now_ns;
    match f.kind {
        Kind::Message => observe_message(h, f, now_ns),
        Kind::Reject => h.rejects += 1,
        Kind::Status => observe_status(h, f.payload),
    }
}

fn observe_seq(h: &mut FeedHealth, seq: u64) {
    if h.frames == 0 || seq == h.seq_next {
        // First frame, or exactly the expected one.
    } else if seq > h.seq_next {
        h.gaps += 1;
        h.missing += seq - h.seq_next;
    } else if seq == h.seq_last {
        h.duplicates += 1;
        return;
    } else {
        h.regressions += 1;
    }
    h.seq_last = seq;
    h.seq_next = seq.saturating_add(1);
}

fn observe_message(h: &mut FeedHealth, f: &Frame<'_>, now_ns: i64) {
    let len = f.payload.len() as u32;
    h.messages += 1;
    h.payload_bytes += u64::from(len);
    h.size_min = h.size_min.min(len);
    h.size_max = h.size_max.max(len);
    let bin = (u32::BITS - len.leading_zeros()) as usize;
    if let Some(c) = h.size_hist.get_mut(bin.min(SIZE_BINS - 1)) {
        *c += 1;
    }
    if f.flags & FLAG_REDELIVERED != 0 {
        h.redelivered += 1;
    }
    let sec = now_ns.div_euclid(NS_PER_SEC);
    if let Some(slot) = h.ring.get_mut(ring_index(sec)) {
        if slot.sec != sec {
            *slot = Slot {
                sec,
                count: 0,
                bytes: 0,
            };
        }
        slot.count = slot.count.saturating_add(1);
        slot.bytes = slot.bytes.saturating_add(len);
    }
    if f.jms_timestamp_ms == 0 {
        h.no_timestamp += 1;
        return;
    }
    let lat_ms = f
        .recv_unix_ns
        .div_euclid(1_000_000)
        .saturating_sub(f.jms_timestamp_ms);
    if lat_ms < 0 {
        h.skew += 1;
    } else {
        let bin = LAT_EDGES_MS.partition_point(|&e| e <= lat_ms);
        if let Some(c) = h.lat_hist.get_mut(bin) {
            *c += 1;
        }
    }
}

fn observe_status(h: &mut FeedHealth, payload: &[u8]) {
    for line in payload.split(|&b| b == b'\n') {
        match line {
            b"state=connected" => h.state = STATE_CONNECTED,
            b"state=disconnected" => h.state = STATE_DISCONNECTED,
            b"state=reconnect_attempt" => {
                h.state = STATE_DISCONNECTED;
                h.reconnects += 1;
            }
            _ => {
                if let Some(v) = line.strip_prefix(b"delay_ms=") {
                    h.last_reconnect_delay_ms = parse_u64(v);
                }
            }
        }
    }
}

fn parse_u64(d: &[u8]) -> u64 {
    d.iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0u64, |a, &b| {
            a.saturating_mul(10).saturating_add(u64::from(b - b'0'))
        })
}

/// Counts a refused frame.
pub fn observe_error(h: &mut FeedHealth, _e: &FrameError) {
    h.frame_errors += 1;
}

/// Sets the skipped-byte figure from the reader's running total.
pub fn observe_skipped(h: &mut FeedHealth, reader_skipped_total: u64) {
    h.skipped_bytes = reader_skipped_total;
}

/// Message rates in milli-messages per second: (last complete second, last up-to-60 seconds).
pub fn rates_milli(h: &FeedHealth, now_ns: i64) -> (u64, u64) {
    if h.frames == 0 {
        return (0, 0);
    }
    let now_sec = now_ns.div_euclid(NS_PER_SEC);
    let first_sec = h.first_recv_ns.div_euclid(NS_PER_SEC);
    let count_at = |sec: i64| -> u64 {
        match h.ring.get(ring_index(sec)) {
            Some(s) if s.sec == sec => u64::from(s.count),
            _ => 0,
        }
    };
    let r1 = count_at(now_sec - 1).saturating_mul(1000);
    let window = (now_sec - first_sec).clamp(1, 60);
    let sum: u64 = (1..=60).map(|back| count_at(now_sec - back)).sum();
    (r1, sum.saturating_mul(1000) / window as u64)
}

/// Upper edge in ms (exclusive) of the bin holding quantile `num/den` of the latency samples;
/// `None` if there are no samples or the quantile falls in the open last bin.
pub fn latency_quantile_ms(h: &FeedHealth, num: u64, den: u64) -> Option<i64> {
    let total: u64 = h.lat_hist.iter().sum();
    if total == 0 || den == 0 {
        return None;
    }
    let target = (total * num).div_ceil(den).max(1);
    let mut cum = 0u64;
    for (i, c) in h.lat_hist.iter().enumerate() {
        cum += c;
        if cum >= target {
            return LAT_EDGES_MS.get(i).copied();
        }
    }
    None
}

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
    let q = |n: u64, d: u64| match (latency_quantile_ms(h, n, d), h.lat_hist.iter().sum::<u64>()) {
        (_, 0) => "-".to_string(),
        (Some(e), _) => format!("<{e}"),
        (None, _) => format!(">={}", LAT_EDGES_MS.last().copied().unwrap_or(0)),
    };
    let mut s = String::with_capacity(256);
    let _ = write!(
        s,
        "state={state} msgs={} rate_1s={}.{:03} rate_60s={}.{:03} bytes={} size_min={size_min} \
         size_max={} lat_p50_ms={} lat_p95_ms={} skew={} gaps={} missing={} dups={} regress={} \
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
        h.redelivered,
        h.rejects,
        h.reconnects,
        h.frame_errors,
        h.skipped_bytes,
    );
    s
}

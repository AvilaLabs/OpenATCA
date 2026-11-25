#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
//! Plain time-window values and pure operations. This crate has no dependency on the private
//! sealed-data guard; tools construct the active window through `oa-sealed` and pass the value to
//! decoders.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("oa-time-window supports 64-bit targets only");

/// A half-open span of Unix milliseconds, `from_ms <= t < to_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
// A7.2: cold decoder configuration passed once per input document.
pub struct DropWindow {
    from_ms: i64,
    to_ms: i64,
}

// Budget: one cold, two-field decoder setting.
const _: () = assert!(core::mem::size_of::<DropWindow>() == 16);

impl DropWindow {
    /// The span `from_ms` (inclusive) to `to_ms` (exclusive).
    pub const fn new(from_ms: i64, to_ms: i64) -> DropWindow {
        DropWindow { from_ms, to_ms }
    }

    /// A window that holds no time; for tests and fuzz targets only.
    pub const fn empty_for_tests_and_fuzz() -> DropWindow {
        DropWindow {
            from_ms: 0,
            to_ms: 0,
        }
    }

    /// A window that holds all representable timestamps.
    pub const fn all_time() -> DropWindow {
        DropWindow {
            from_ms: i64::MIN,
            to_ms: i64::MAX,
        }
    }
}

/// Convert nanoseconds to a millisecond window, rounding outward so no instant is uncovered.
pub const fn from_ns_span(from_ns: i64, to_ns: i64) -> DropWindow {
    let up = i64::div_euclid(to_ns, 1_000_000) + (i64::rem_euclid(to_ns, 1_000_000) != 0) as i64;
    DropWindow::new(i64::div_euclid(from_ns, 1_000_000), up)
}

/// Whether `t_ms` is in `window`.
pub const fn contains(window: DropWindow, t_ms: i64) -> bool {
    window.from_ms <= t_ms && t_ms < window.to_ms
}

/// The span, or `None` when the window is empty or inverted.
pub const fn span(window: DropWindow) -> Option<(i64, i64)> {
    if window.from_ms < window.to_ms {
        Some((window.from_ms, window.to_ms))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{contains, from_ns_span, span, DropWindow};

    // REQ: OA-CORE-007
    #[test]
    fn half_open_and_empty_windows_keep_their_boundaries() {
        let w = DropWindow::new(-2, 3);
        assert_eq!(span(w), Some((-2, 3)));
        assert!(!contains(w, -3));
        assert!(contains(w, -2));
        assert!(contains(w, 2));
        assert!(!contains(w, 3));

        let empty = DropWindow::empty_for_tests_and_fuzz();
        assert_eq!(span(empty), None);
        assert!(!contains(empty, 0));
    }

    #[test]
    fn nanoseconds_round_outward_even_before_epoch() {
        assert_eq!(from_ns_span(1, 1), DropWindow::new(0, 1));
        assert_eq!(from_ns_span(-1, -1), DropWindow::new(-1, 0));
        assert_eq!(from_ns_span(-1_000_001, 1_000_001), DropWindow::new(-2, 2));
    }
}

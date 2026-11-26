# Time-window value

**Status: Preview supporting API proposal.** `oa-time-window` is a 64-bit-target, `no_std` value
crate. The decoder receives a caller-constructed window; this module does not choose a policy,
read a clock, inspect files, or authenticate a time source.

## Source-backed surface

`DropWindow` is a 16-byte `#[repr(C)]` pair of signed Unix-millisecond boundaries. The endpoints
are private; `DropWindow::new(from_ms, to_ms)` preserves the supplied values. The interval is
half-open: `from_ms <= t_ms < to_ms`. `span` returns `None` for empty or inverted boundaries, and
`contains` returns false for an inverted range.

`from_ns_span(from_ns, to_ns)` converts signed Unix nanoseconds to milliseconds, flooring the
inclusive start and rounding the exclusive end upward so that the converted span does not omit a
represented instant. `empty_for_tests_and_fuzz` and `all_time` are explicit constructors. All units
are Unix-relative; no UTC calendar conversion is performed here.

The source all_time constructor stores the half-open span [i64::MIN, i64::MAX), so it excludes
the maximum representable i64 timestamp despite its source comment describing all timestamps.

## Limits and release boundary

The crate currently rejects targets whose pointer width is not 64 bits at compile time. The type
is a decoder configuration value, not a general duration library, time-zone API, monotonic clock,
or policy source. A caller that supplies an inverted window receives an empty span rather than
normalization. The crate is a supporting dependency for decoder packages, not a separate feature
commitment in the proposed 0.1 format list. The operation remains Preview until its public API,
target policy, and shared test-vector coverage are reviewed.

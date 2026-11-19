# oa-math

Elementary functions that give the same bits on every IEEE-754 machine (see `src/lib.rs` for the
method and the measured accuracy).

## `no_std`, and where the square root comes from

The library is `#![cfg_attr(not(test), no_std)]` (tests keep `std`), so the compiler itself
refuses file, network, clock, thread and hash-map use in it. `oa-geo` is built the same way.

On stable Rust 1.89, `f64::sqrt`, `floor`, `ceil`, `trunc` and `round` exist only in `std`.
`core::arch` has the hardware square root (`_mm_sqrt_sd` on x86-64, `vsqrtd_f64` on aarch64,
`f64_sqrt` on wasm32), but every one of those intrinsics is a `#[target_feature]` function, and
calling it from ordinary code needs `unsafe` even where the feature is on by default; RULESET A0
forbids `unsafe`. (Checked on x86-64 with rustc 1.89 only; the aarch64 and wasm32 targets were not
installed.) A software square root (bit by bit on the significand) is correct but about
140 times slower than the hardware instruction.

So the operations come from the small crate `oa-ieee`, which is `std` and has exactly five public
functions (`sqrt`, `floor`, `ceil`, `trunc`, `round`), each a one-line call of the std method.
IEEE-754 requires square root to be correctly rounded and the other four are exact, so every
conforming machine returns the same bits for every result that is not a NaN. (A NaN result, such as the square root of a negative number, is required to be a NaN but not which one: its sign and payload differ between machines, so `oa_math::canon_nan` maps it to the one fixed NaN before its bits are hashed or written.) The determinism rule is unchanged.

One caveat: because `oa-ieee` is in the dependency graph, the compiler still resolves the
std-only inherent methods on `f64` (`x.sqrt()`) in these crates. Paths into `std` itself
(`std::fs`, `std::time`, ...) are refused, but the method calls are not, so `tests/purity.rs`
(here) and `tests/general.rs` (in `oa-geo`) scan the sources and fail on them.
